//! Point-in-time change descriptions. Reading and generation never stage or write repository files.
use crate::{
    codex::Provider,
    commit_agent::{self, CommitControl},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    fmt::Write as _,
    io::{Read, Seek, SeekFrom},
    path::{Component, Path},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

const COMMAND_LIMIT: u64 = 8 * 1024 * 1024;
const FILE_LIMIT: usize = 1000;
const PATCH_BUDGET: usize = 120_000;
const FILE_PATCH_LIMIT: usize = 24_000;
const SCHEMA: &str = r#"{"type":"object","properties":{"title":{"type":"string"},"types":{"type":"array","items":{"type":"string"}},"summary":{"type":"array","items":{"type":"string"}},"files":{"type":"array","items":{"type":"object","properties":{"path":{"type":"string"},"title":{"type":"string"},"summary":{"type":"array","items":{"type":"string"}},"category":{"type":"string"}},"required":["path","title","summary","category"],"additionalProperties":false}},"content":{"type":"string"},"diagram":{"type":["string","null"]}},"required":["title","types","summary","files","content","diagram"],"additionalProperties":false}"#;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Scope {
    Local,
    PullRequest { url: String },
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SnapshotFile {
    pub path: String,
    pub added: u32,
    pub removed: u32,
    pub patch: String,
    /// Files without a complete supplied patch must not be claimed as analyzed.
    pub omitted: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Snapshot {
    pub scope: Scope,
    pub fingerprint: String,
    pub label: String,
    pub head: String,
    pub files: Vec<SnapshotFile>,
    pub original_body: Option<String>,
    pub title: String,
    pub base: String,
    pub checkout: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FileDescription {
    pub path: String,
    pub title: String,
    pub summary: Vec<String>,
    pub category: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Description {
    pub snapshot: Snapshot,
    pub title: String,
    pub types: Vec<String>,
    pub summary: Vec<String>,
    pub files: Vec<FileDescription>,
    /// Optional custom prose rendered by Grove's native Markdown components.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub content: String,
    pub diagram: Option<String>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Reading,
    Generating,
}

impl Description {
    pub fn markdown(&self) -> String {
        let mut text = format!(
            "### PR Type\n\n{}\n\n### Description\n\n{}\n\n",
            self.types.join(", "),
            bullets(&self.summary)
        );
        if !self.content.is_empty() {
            let _ = writeln!(text, "{}\n", self.content);
        }
        text.push_str("### File walkthrough\n\n");
        for file in &self.files {
            let _ = writeln!(
                text,
                "- `{}`: {}\n{}",
                file.path.replace('`', "'"),
                file.title,
                file.summary
                    .iter()
                    .map(|line| format!("  - {line}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            );
        }
        if let Some(diagram) = &self.diagram {
            let _ = writeln!(text, "\n### Processing flow\n\n```mermaid\n{diagram}\n```");
        }
        let _ = writeln!(text, "\n{} of {} files described. Based on the diff; tests were not executed by this description.", self.files.len(), self.snapshot.files.len());
        for file in &self.snapshot.files {
            if !self
                .files
                .iter()
                .any(|described| described.path == file.path)
            {
                let reason = file
                    .omitted
                    .as_deref()
                    .unwrap_or("Not returned by the description model");
                let _ = writeln!(
                    text,
                    "- Omitted `{}`: {reason}.",
                    file.path.replace('`', "'")
                );
            }
        }
        text
    }
    /// Preserve original human context without duplicating generated sections on repeated updates.
    pub fn pr_body(&self) -> String {
        // Model-authored text must not introduce section delimiters: a later
        // update locates these exact comments to preserve the human PR context.
        let markdown = self
            .markdown()
            .replace(
                "<!-- grove:description:start -->",
                "&lt;!-- grove:description:start --&gt;",
            )
            .replace(
                "<!-- grove:description:end -->",
                "&lt;!-- grove:description:end --&gt;",
            );
        let generated =
            format!("<!-- grove:description:start -->\n{markdown}\n<!-- grove:description:end -->");
        let original = self.snapshot.original_body.as_deref().unwrap_or_default();
        if let (Some(start), Some(end)) = (
            original.find("<!-- grove:description:start -->"),
            original.find("<!-- grove:description:end -->"),
        ) {
            if start <= end {
                return format!(
                    "{}{}{}",
                    &original[..start],
                    generated,
                    &original[end + "<!-- grove:description:end -->".len()..]
                );
            }
        }
        if original.trim().is_empty() {
            generated
        } else {
            format!("{generated}\n\n{original}")
        }
    }
}
fn bullets(lines: &[String]) -> String {
    lines
        .iter()
        .map(|line| format!("- {line}"))
        .collect::<Vec<_>>()
        .join("\n")
}
fn digest(bytes: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, bytes)
        .as_ref()
        .iter()
        .fold(String::with_capacity(64), |mut text, byte| {
            let _ = write!(text, "{byte:02x}");
            text
        })
}
fn path_valid(name: &str) -> bool {
    !name.is_empty()
        && Path::new(name)
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
}
fn cancel(control: &CommitControl) -> Result<(), String> {
    if control.canceled() {
        Err("Description generation canceled".into())
    } else {
        Ok(())
    }
}

/// Read a complete change identity and a bounded set of patches. Never stages or fetches.
pub fn prepare(path: &Path, scope: &Scope) -> Result<Snapshot, String> {
    prepare_with_control(path, scope, None)
}
fn prepare_with_control(
    path: &Path,
    scope: &Scope,
    control: Option<&CommitControl>,
) -> Result<Snapshot, String> {
    check_control(control)?;
    let checkout = fs_err::canonicalize(path)
        .map_err(|_| "Checkout is unavailable")?
        .to_string_lossy()
        .into_owned();
    let path = Path::new(&checkout);
    let read = Capture { path, control };
    match scope {
        Scope::Local => local_snapshot(&read, checkout.clone()),
        Scope::PullRequest { url } => pr_snapshot(&read, checkout.clone(), url),
    }
}
pub fn generate(
    path: &Path,
    scope: &Scope,
    provider: Provider,
    control: &CommitControl,
    mut progress: impl FnMut(Phase),
) -> Result<Description, String> {
    cancel(control)?;
    progress(Phase::Reading);
    let snapshot = prepare_with_control(path, scope, Some(control))?;
    generate_snapshot(snapshot, provider, control, &mut progress)
}
/// Separate capture and generation for callers which already show the immutable scope.
pub fn generate_snapshot(
    snapshot: Snapshot,
    provider: Provider,
    control: &CommitControl,
    mut progress: impl FnMut(Phase),
) -> Result<Description, String> {
    cancel(control)?;
    let prompt = generation_prompt(
        &snapshot,
        "Return the required JSON object and nothing else.",
    )?;
    progress(Phase::Generating);
    let value =
        commit_agent::generate_json(provider, &prompt, SCHEMA, control).map_err(|error| {
            error
                .replace("commit message", "change description")
                .replace("commit model", "description model")
                .replace("commit result", "description result")
                .replace("commit schema", "description schema")
                .replace("structured commit", "structured description")
                .replace("Commit assistant", "Description assistant")
                .replace("commit assistant", "description assistant")
                .replace("Commit canceled", "Description generation canceled")
        })?;
    cancel(control)?;
    validate(snapshot, value)
}

fn generation_prompt(snapshot: &Snapshot, output_instruction: &str) -> Result<String, String> {
    if snapshot.files.is_empty() {
        return Err("There are no changes to describe in this scope".into());
    }
    let available: Vec<_> = snapshot
        .files
        .iter()
        .filter(|file| file.omitted.is_none())
        .collect();
    if available.is_empty() {
        return Err("No text patches fit the description limits. Review binary or oversized files in the diff".into());
    }
    let data = serde_json::to_string(&serde_json::json!({"scope":snapshot.label,"previous_title":snapshot.title,"files":available}))
        .map_err(|_| "Could not prepare description input")?;
    let context = format!("{output_instruction}\n\nRequired result fields: title, types (one or more of Bug fix, Tests, Enhancement, Documentation, Other), summary (1-4 nonempty single-line bullets, up to 500 characters each), files (exact captured path, title, summary and category for each described complete text patch), content (optional extended Markdown, empty when unneeded), diagram (optional concise Mermaid flowchart source, no Markdown fences, or null). Never invent paths or describe omitted patches. Grove renders content through native selectable Markdown: headings, paragraphs, lists, tables, blockquotes, fenced code and file links. Use these components where useful, and the diagram field for Mermaid; do not return HTML, CSS, scripts, or executable UI. Keep content within 64 KiB. The required snapshot metadata and coverage stay visible outside custom content. Do not claim tests passed or executed.\n\n<change-data>\n{data}\n</change-data>");
    Ok(crate::prompts::compose(
        crate::prompts::PromptKind::DescribeChanges,
        &context,
    ))
}

/// Build the initial prompt for a real Codex or Claude conversation from the
/// captured diff. This does not call a model or grant tools or write permissions.
pub fn conversation_prompt(snapshot: &Snapshot) -> Result<String, String> {
    let instruction = format!(
        "Briefly summarize the result for the conversation, then include exactly one complete \
         `grove-description` fenced code block containing the required JSON object. \
         Use a line containing ```grove-description to open the block and a line containing \
         ``` to close it. Do not include another description block. The JSON must conform \
         to this schema: {SCHEMA}. Grove validates and saves the result and renders its native page."
    );
    generation_prompt(snapshot, &instruction)
}

/// Import a completed assistant response from either provider. Only a complete,
/// unambiguous structured result for the captured snapshot becomes a description.
pub fn from_conversation(snapshot: Snapshot, text: &str) -> Result<Description, String> {
    const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
    if text.len() > MAX_RESPONSE_BYTES {
        return Err("Description response exceeds the import limit. Retry generation".into());
    }
    let text = text.trim();
    let payload = if text.starts_with('{') {
        text.to_owned()
    } else {
        let mut block = None;
        let mut current = None::<String>;
        let mut other_fence = false;
        for line in text.lines() {
            let marker = line.trim();
            if let Some(content) = &mut current {
                if marker == "```" {
                    block = current.take();
                } else if marker.starts_with("```") {
                    return Err("Description response contains an invalid or duplicate block. Retry generation".into());
                } else {
                    content.push_str(line);
                    content.push('\n');
                }
            } else if other_fence {
                if marker == "```" {
                    other_fence = false;
                }
            } else if marker == "```grove-description" {
                if block.is_some() {
                    return Err(
                        "Description response contains multiple result blocks. Retry generation"
                            .into(),
                    );
                }
                current = Some(String::new());
            } else if marker.starts_with("```grove-description") {
                return Err(
                    "Description response has an invalid result block marker. Retry generation"
                        .into(),
                );
            } else if marker.starts_with("```") {
                other_fence = true;
            }
        }
        if current.is_some() {
            return Err(
                "Description response has an incomplete result block. Retry generation".into(),
            );
        }
        block.ok_or("Description response has no complete result block. Retry generation")?
    };
    let value = serde_json::from_str(&payload)
        .map_err(|_| "Description response contains invalid JSON. Retry generation")?;
    validate(snapshot, value)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Prediction {
    title: String,
    types: Vec<String>,
    summary: Vec<String>,
    files: Vec<FileDescription>,
    #[serde(default)]
    content: String,
    diagram: Option<String>,
}
fn valid_text(value: &str, max: usize) -> bool {
    !value.trim().is_empty()
        && value.chars().count() <= max
        && !value.chars().any(|c| c.is_control() && c != '\t')
}
fn valid_lines(lines: &[String]) -> bool {
    (1..=4).contains(&lines.len()) && lines.iter().all(|line| valid_text(line, 500))
}
pub(crate) fn validate(
    snapshot: Snapshot,
    value: serde_json::Value,
) -> Result<Description, String> {
    let prediction: Prediction = serde_json::from_value(value)
        .map_err(|_| "Description assistant returned invalid structured output")?;
    if !valid_text(&prediction.title, 200)
        || !valid_lines(&prediction.summary)
        || prediction.types.is_empty()
        || prediction.types.len() > 5
        || prediction.types.iter().any(|kind| {
            !["Bug fix", "Tests", "Enhancement", "Documentation", "Other"].contains(&kind.as_str())
        })
    {
        return Err("Description assistant returned an invalid title, summary, or change type. Retry generation".into());
    }
    let mut seen = HashSet::new();
    for file in &prediction.files {
        if !seen.insert(&file.path)
            || !snapshot
                .files
                .iter()
                .any(|source| source.path == file.path && source.omitted.is_none())
            || !valid_text(&file.title, 200)
            || !valid_lines(&file.summary)
            || !valid_text(&file.category, 64)
        {
            return Err("Description assistant returned an unknown, duplicate, or invalid file. Retry generation".into());
        }
    }
    if prediction.files.is_empty() {
        return Err("Description assistant did not describe any files. Retry generation".into());
    }
    if prediction.content.len() > 64 * 1024
        || prediction
            .content
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
    {
        return Err(
            "Description assistant returned invalid or oversized content. Retry generation".into(),
        );
    }
    let diagram = prediction
        .diagram
        .filter(|diagram| !diagram.trim().is_empty());
    if diagram.as_ref().is_some_and(|diagram| {
        diagram.len() > 8000
            || !diagram.trim_start().starts_with("flowchart ")
            || diagram.contains("```")
            || diagram.contains("%%{")
            || diagram.contains("click ")
    }) {
        return Err("Description assistant returned an invalid diagram. Retry generation".into());
    }
    Ok(Description {
        snapshot,
        title: prediction.title,
        types: prediction.types,
        summary: prediction.summary,
        files: prediction.files,
        content: if prediction.content.trim().is_empty() {
            String::new()
        } else {
            prediction.content
        },
        diagram,
    })
}

/// Snapshot freshness is checked against complete content identities, not timestamps or counts.
pub fn is_current(path: &Path, description: &Description) -> Result<bool, String> {
    let checkout = fs_err::canonicalize(path).map_err(|_| "Checkout is unavailable")?;
    match &description.snapshot.scope {
        Scope::Local => {
            if checkout.to_string_lossy() != description.snapshot.checkout {
                return Ok(false);
            }
            let read = Capture {
                path: &checkout,
                control: None,
            };
            let names = git_files(&read)?;
            let head = git(&checkout, &["rev-parse", "--verify", "HEAD"], 1024).unwrap_or_default();
            Ok(local_identity(&read, &names, head.trim())? == description.snapshot.fingerprint)
        }
        Scope::PullRequest { url } => {
            pr_identity(url)?;
            let current = pr_info(
                &Capture {
                    path: &checkout,
                    control: None,
                },
                url,
            )?;
            Ok(current.head_ref_oid == description.snapshot.head
                && current.base_ref_oid == description.snapshot.base
                && Some(current.body) == description.snapshot.original_body)
        }
    }
}
/// Update only the body, after an explicit native confirmation. Never changes the title.
pub fn publish(path: &Path, description: &Description, edited_body: &str) -> Result<(), String> {
    if edited_body.trim().is_empty() || edited_body.len() > 65_536 || edited_body.contains('\0') {
        return Err("Enter a PR description of at most 64 KiB before updating".into());
    }
    let Scope::PullRequest { url } = &description.snapshot.scope else {
        return Err("Attach a pull request to publish a description".into());
    };
    let checkout = fs_err::canonicalize(path).map_err(|_| "Checkout is unavailable")?;
    if checkout.to_string_lossy() != description.snapshot.checkout {
        return Err("Description belongs to another checkout".into());
    }
    pr_identity(url)?;
    let current = pr_info(
        &Capture {
            path: &checkout,
            control: None,
        },
        url,
    )?;
    if current.head_ref_oid != description.snapshot.head
        || current.base_ref_oid != description.snapshot.base
    {
        return Err("PR changes have moved on. Regenerate the description before updating".into());
    }
    if Some(current.body) != description.snapshot.original_body {
        return Err(
            "The PR description changed since this preview. Regenerate before replacing it".into(),
        );
    }
    let (owner, repository, number) = pr_identity(url)?;
    let mut body =
        tempfile::NamedTempFile::new().map_err(|_| "Could not prepare PR description")?;
    std::io::Write::write_all(
        &mut body,
        serde_json::to_string(&serde_json::json!({"body":edited_body}))
            .map_err(|_| "Could not prepare PR description")?
            .as_bytes(),
    )
    .map_err(|_| "Could not prepare PR description")?;
    let body_path = body
        .path()
        .to_str()
        .ok_or("Could not prepare PR description path")?;
    command(
        path,
        "gh",
        &[
            "api",
            "--hostname",
            "github.com",
            "--method",
            "PATCH",
            &format!("repos/{owner}/{repository}/pulls/{number}"),
            "--input",
            body_path,
        ],
        COMMAND_LIMIT,
    )?;
    Ok(())
}

struct Capture<'a> {
    path: &'a Path,
    control: Option<&'a CommitControl>,
}
impl Capture<'_> {
    fn git(&self, args: &[&str], limit: u64) -> Result<String, String> {
        self.command("git", args, limit)
    }
    fn command(&self, program: &str, args: &[&str], limit: u64) -> Result<String, String> {
        command_with_control(self.path, program, args, limit, self.control)
    }
}
fn check_control(control: Option<&CommitControl>) -> Result<(), String> {
    control.map_or(Ok(()), cancel)
}
fn git(path: &Path, args: &[&str], limit: u64) -> Result<String, String> {
    command(path, "git", args, limit)
}
fn command(path: &Path, program: &str, args: &[&str], limit: u64) -> Result<String, String> {
    command_with_control(path, program, args, limit, None)
}
fn command_with_control(
    path: &Path,
    program: &str,
    args: &[&str],
    limit: u64,
    control: Option<&CommitControl>,
) -> Result<String, String> {
    check_control(control)?;
    let mut output = tempfile::tempfile().map_err(|_| "Could not prepare change snapshot")?;
    let errors = tempfile::tempfile().map_err(|_| "Could not prepare change snapshot")?;
    let executable = crate::agent::resolve_executable(program)
        .ok_or_else(|| format!("Install {program} to describe these changes"))?;
    let mut command = Command::new(executable);
    command.current_dir(path);
    if let Some(environment) = crate::env_path::session_environment() {
        command.envs(environment.iter().cloned());
    }
    if program == "git" {
        command.args(["-c", "core.fsmonitor=false", "-c", "core.quotePath=false"]);
    }
    command
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GH_PROMPT_DISABLED", "1")
        .env("GH_PAGER", "cat")
        .stdin(Stdio::null())
        .stdout(Stdio::from(
            output
                .try_clone()
                .map_err(|_| "Could not prepare snapshot output")?,
        ))
        .stderr(Stdio::from(
            errors
                .try_clone()
                .map_err(|_| "Could not prepare snapshot errors")?,
        ));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command
        .spawn()
        .map_err(|_| format!("Could not start {program} for the change snapshot"))?;
    let started = Instant::now();
    loop {
        if control.is_some_and(CommitControl::canceled) {
            #[cfg(unix)]
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGKILL);
            }
            let _ = child.kill();
            let _ = child.wait();
            return Err("Description generation canceled".into());
        }
        let excessive = output.metadata().map_or(true, |meta| meta.len() > limit)
            || errors.metadata().map_or(true, |meta| meta.len() > 65_536);
        match child.try_wait() {
            Ok(Some(status)) if !excessive => {
                if !status.success() {
                    return Err(if program == "gh" { "GitHub changes unavailable. Check gh authentication, network access, and PR permissions" } else { "Could not read the change snapshot. Refresh the checkout and retry" }.into());
                }
                output
                    .seek(SeekFrom::Start(0))
                    .map_err(|_| "Could not read snapshot output")?;
                let mut text = String::new();
                output
                    .take(limit + 1)
                    .read_to_string(&mut text)
                    .map_err(|_| "Change snapshot contains unsupported non-UTF-8 data")?;
                if text.len() as u64 > limit {
                    return Err("Change snapshot exceeds the size limit".into());
                }
                return Ok(text);
            }
            Ok(None) if !excessive && started.elapsed() < Duration::from_secs(30) => {
                std::thread::sleep(Duration::from_millis(20));
            }
            _ => {
                #[cfg(unix)]
                unsafe {
                    libc::kill(-(child.id() as i32), libc::SIGKILL);
                }
                let _ = child.kill();
                let _ = child.wait();
                return Err(if excessive {
                    "Change snapshot exceeds the size limit"
                } else {
                    "Change snapshot timed out; check repository access and retry"
                }
                .into());
            }
        }
    }
}
fn git_files(read: &Capture<'_>) -> Result<Vec<String>, String> {
    check_control(read.control)?;
    let mut names: Vec<_> = read
        .git(
            &[
                "ls-files",
                "--modified",
                "--deleted",
                "--others",
                "--exclude-standard",
                "-z",
            ],
            COMMAND_LIMIT,
        )?
        .split('\0')
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .collect();
    names.extend(
        read.git(
            &["diff", "--cached", "--name-only", "--no-renames", "-z"],
            COMMAND_LIMIT,
        )?
        .split('\0')
        .filter(|name| !name.is_empty())
        .map(str::to_owned),
    );
    names.sort();
    names.dedup();
    if names.len() > FILE_LIMIT {
        return Err(
            "More than 1,000 files changed. Narrow the change set before describing it".into(),
        );
    }
    if names.iter().any(|name| !path_valid(name)) {
        return Err("A changed file has an unsupported path".into());
    }
    Ok(names)
}
fn contained(path: &Path, name: &str) -> Result<(), String> {
    let mut parent = path
        .join(name)
        .parent()
        .map(Path::to_path_buf)
        .ok_or("Invalid changed file path")?;
    while !parent.exists() {
        if !parent.pop() {
            return Err("Changed path is unavailable".into());
        }
    }
    let real =
        fs_err::canonicalize(parent).map_err(|_| "Could not inspect changed file ancestry")?;
    if !real.starts_with(path) {
        return Err("A changed path traverses a symlink outside the checkout".into());
    }
    Ok(())
}
fn local_identity(read: &Capture<'_>, names: &[String], head: &str) -> Result<String, String> {
    let path = read.path;
    check_control(read.control)?;
    let mut bytes = format!("{}\0{head}\0", path.display()).into_bytes();
    bytes.extend_from_slice(
        read.git(&["ls-files", "--stage", "-z"], COMMAND_LIMIT)?
            .as_bytes(),
    );
    for name in names {
        contained(path, name)?;
        bytes.extend_from_slice(name.as_bytes());
        bytes.push(0);
        match fs_err::symlink_metadata(path.join(name)) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                bytes.extend_from_slice(b"symlink\0");
                bytes.extend_from_slice(
                    fs_err::read_link(path.join(name))
                        .map_err(|_| "Could not inspect a changed symlink")?
                        .as_os_str()
                        .as_encoded_bytes(),
                );
            }
            Ok(metadata) if metadata.is_file() => {
                bytes.extend_from_slice(b"file\0");
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    bytes.extend_from_slice(&metadata.permissions().mode().to_le_bytes());
                }
                bytes.extend_from_slice(
                    read.git(&["hash-object", "--no-filters", "--", name], 1024)?
                        .as_bytes(),
                );
            }
            Ok(_) => {
                bytes.extend_from_slice(b"directory\0");
                bytes.extend_from_slice(
                    read.git(
                        &[
                            "diff",
                            "--raw",
                            "--no-ext-diff",
                            "--no-textconv",
                            "HEAD",
                            "--",
                            name,
                        ],
                        65_536,
                    )?
                    .as_bytes(),
                );
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                bytes.extend_from_slice(b"deleted\0");
            }
            Err(_) => return Err("Could not inspect a changed file".into()),
        }
        bytes.push(0);
    }
    Ok(digest(&bytes))
}
fn local_snapshot(read: &Capture<'_>, checkout: String) -> Result<Snapshot, String> {
    let path = read.path;
    check_control(read.control)?;
    let head = read
        .git(&["rev-parse", "--verify", "HEAD"], 1024)
        .unwrap_or_default()
        .trim()
        .to_owned();
    let names = git_files(read)?;
    let fingerprint = local_identity(read, &names, &head)?;
    let mut remaining = PATCH_BUDGET;
    let mut files = Vec::new();
    for name in &names {
        check_control(read.control)?;
        contained(path, name)?;
        let metadata = fs_err::symlink_metadata(path.join(name));
        let indexed = read
            .git(&["ls-files", "--error-unmatch", "--", name], 65_536)
            .is_ok();
        let in_head = !head.is_empty()
            && read
                .git(&["cat-file", "-e", &format!("{head}:{name}")], 1024)
                .is_ok();
        check_control(read.control)?;
        let untracked = (!indexed && !in_head) || head.is_empty();
        let mut file = SnapshotFile {
            path: name.clone(),
            added: 0,
            removed: 0,
            patch: String::new(),
            omitted: None,
        };
        if metadata.as_ref().is_ok_and(|m| !m.is_file()) {
            file.omitted = Some("Symlink or submodule content is not included".into());
        } else if untracked {
            if metadata
                .as_ref()
                .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
            {
                file.omitted = Some("No net text content against the empty initial tree".into());
            } else if metadata
                .as_ref()
                .is_ok_and(|meta| meta.len() > FILE_PATCH_LIMIT as u64)
            {
                file.omitted = Some("File exceeds the per-file patch limit".into());
            } else {
                match read_untracked_bounded(&path.join(name))? {
                    None => file.omitted = Some("File exceeds the per-file patch limit".into()),
                    Some(bytes) => match String::from_utf8(bytes) {
                        Ok(text) if !text.contains('\0') => {
                            file.added = text.lines().count() as u32;
                            file.patch = text.lines().fold(String::new(), |mut patch, line| {
                                let _ = writeln!(patch, "+{line}");
                                patch
                            });
                        }
                        _ => file.omitted = Some("Binary content is not included".into()),
                    },
                }
            }
        } else {
            let mut args = vec![
                "diff",
                "--no-ext-diff",
                "--no-textconv",
                "--no-renames",
                "--unified=3",
            ];
            if !head.is_empty() {
                args.push(&head);
            } else {
                args.push("--cached");
            }
            args.extend(["--", name]);
            match read.git(&args, FILE_PATCH_LIMIT as u64) {
                Ok(patch)
                    if patch.contains("Binary files ") || patch.contains("GIT binary patch") =>
                {
                    file.omitted = Some("Binary content is not included".into());
                }
                Ok(patch) => {
                    file.added = patch
                        .lines()
                        .filter(|line| line.starts_with('+') && !line.starts_with("+++ "))
                        .count() as u32;
                    file.removed = patch
                        .lines()
                        .filter(|line| line.starts_with('-') && !line.starts_with("--- "))
                        .count() as u32;
                    file.patch = patch;
                    if file.patch.is_empty() {
                        file.omitted = Some("No net text changes against HEAD".into());
                    }
                }
                Err(error) if error.contains("size limit") => {
                    file.omitted = Some("File exceeds the per-file patch limit".into());
                }
                Err(error) => return Err(error),
            }
        }
        apply_budget(&mut file, &mut remaining);
        files.push(file);
    }
    check_control(read.control)?;
    let current_identity = local_identity(read, &git_files(read)?, &head)?;
    let current_head = read
        .git(&["rev-parse", "--verify", "HEAD"], 1024)
        .unwrap_or_default();
    check_control(read.control)?;
    if current_head.trim() != head {
        return Err("HEAD changed while reading the snapshot. Retry generation".into());
    }
    if fingerprint != current_identity {
        return Err("Changes moved while reading the snapshot. Retry generation".into());
    }
    Ok(Snapshot {
        scope: Scope::Local,
        fingerprint,
        label: "Local changes · staged + unstaged + untracked".into(),
        head,
        files,
        original_body: None,
        title: String::new(),
        base: String::new(),
        checkout,
    })
}
/// The open handle owns the regular-file check and byte limit; pathname metadata
/// is only an optimization and cannot authorize an unbounded or symlink read.
fn read_untracked_bounded(path: &Path) -> Result<Option<Vec<u8>>, String> {
    let mut options = fs_err::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use fs_err::os::unix::fs::OpenOptionsExt;
        // Do not follow a final-component symlink swapped in after the initial
        // metadata check. NONBLOCK also prevents a swapped FIFO from hanging open.
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path).map_err(|_| {
        "An untracked file changed or became unavailable while reading its snapshot"
    })?;
    if !file
        .metadata()
        .map_err(|_| "Could not inspect the opened untracked file")?
        .is_file()
    {
        return Err("An untracked path is no longer a regular file".into());
    }
    let mut bytes = Vec::new();
    file.take((FILE_PATCH_LIMIT + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| "Could not read an untracked file")?;
    if bytes.len() > FILE_PATCH_LIMIT {
        Ok(None)
    } else {
        Ok(Some(bytes))
    }
}
fn apply_budget(file: &mut SnapshotFile, remaining: &mut usize) {
    if file.omitted.is_none() && file.patch.len() > FILE_PATCH_LIMIT {
        file.omitted = Some("File exceeds the per-file patch limit".into());
    }
    if file.omitted.is_none() && file.patch.len() > *remaining {
        file.omitted = Some("Description context budget reached".into());
    }
    if file.omitted.is_some() {
        file.patch.clear();
    } else {
        *remaining -= file.patch.len();
    }
}
pub(crate) fn pr_identity(value: &str) -> Result<(String, String, u64), String> {
    let url = url::Url::parse(value).map_err(|_| "Invalid GitHub pull request URL")?;
    if url.scheme() != "https"
        || url.host_str() != Some("github.com")
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("Use an https://github.com/owner/repository/pull/number URL".into());
    }
    let segments: Vec<_> = url
        .path_segments()
        .ok_or("Invalid PR path")?
        .filter(|s| !s.is_empty())
        .collect();
    if segments.len() != 4
        || segments[2] != "pull"
        || segments[..2].iter().any(|s| {
            s.is_empty()
                || *s == "."
                || *s == ".."
                || !s
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        })
    {
        return Err("Invalid GitHub pull request URL".into());
    }
    let number = segments[3]
        .parse::<u64>()
        .ok()
        .filter(|n| *n > 0)
        .ok_or("Invalid pull request number")?;
    Ok((segments[0].into(), segments[1].into(), number))
}
#[derive(Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct PrInfo {
    title: String,
    body: String,
    head_ref_oid: String,
    base_ref_oid: String,
    changed_files: usize,
    state: String,
}
fn pr_info(read: &Capture<'_>, url: &str) -> Result<PrInfo, String> {
    check_control(read.control)?;
    serde_json::from_str(&read.command(
        "gh",
        &[
            "pr",
            "view",
            url,
            "--json",
            "title,body,headRefOid,baseRefOid,changedFiles,state",
        ],
        COMMAND_LIMIT,
    )?)
    .map_err(|_| "GitHub returned invalid PR metadata".into())
}
#[derive(Deserialize)]
struct PrFile {
    filename: String,
    additions: u32,
    deletions: u32,
    patch: Option<String>,
}
fn pr_snapshot(read: &Capture<'_>, checkout: String, url: &str) -> Result<Snapshot, String> {
    check_control(read.control)?;
    let (owner, repository, number) = pr_identity(url)?;
    let info = pr_info(read, url)?;
    if info.changed_files > FILE_LIMIT {
        return Err("More than 1,000 files changed. Narrow the PR before describing it".into());
    }
    let response = read.command(
        "gh",
        &[
            "api",
            "--hostname",
            "github.com",
            &format!("repos/{owner}/{repository}/pulls/{number}/files?per_page=100"),
            "--paginate",
            "--slurp",
        ],
        COMMAND_LIMIT,
    )?;
    let pages: Vec<Vec<PrFile>> =
        serde_json::from_str(&response).map_err(|_| "GitHub returned invalid PR file data")?;
    let mut files = Vec::new();
    let mut remaining = PATCH_BUDGET;
    let mut names = HashSet::new();
    for page in pages {
        for file in page {
            if !path_valid(&file.filename) || !names.insert(file.filename.clone()) {
                return Err("GitHub returned an unsupported or duplicate PR path".into());
            }
            let mut source = SnapshotFile {
                path: file.filename,
                added: file.additions,
                removed: file.deletions,
                patch: file.patch.clone().unwrap_or_default(),
                omitted: if file.patch.is_none() {
                    Some("GitHub did not supply a text patch (binary or oversized file)".into())
                } else {
                    None
                },
            };
            if source.omitted.is_none() {
                let additions = source
                    .patch
                    .lines()
                    .filter(|line| line.starts_with('+'))
                    .count();
                let deletions = source
                    .patch
                    .lines()
                    .filter(|line| line.starts_with('-'))
                    .count();
                if additions != source.added as usize || deletions != source.removed as usize {
                    source.omitted = Some("GitHub supplied an incomplete text patch".into());
                }
            }
            apply_budget(&mut source, &mut remaining);
            files.push(source);
        }
    }
    if files.len() != info.changed_files {
        return Err(
            "GitHub returned an incomplete PR file list. Retry after checking PR size and access"
                .into(),
        );
    }
    if pr_info(read, url)? != info {
        return Err("The PR changed while reading its snapshot. Retry generation".into());
    }
    let fingerprint = digest(
        format!(
            "{checkout}\0{url}\0{}\0{}",
            info.head_ref_oid, info.base_ref_oid
        )
        .as_bytes(),
    );
    Ok(Snapshot {
        scope: Scope::PullRequest { url: url.into() },
        fingerprint,
        label: format!("PR #{number}"),
        head: info.head_ref_oid,
        base: info.base_ref_oid,
        files,
        original_body: Some(info.body),
        title: info.title,
        checkout,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    fn git_test(path: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .current_dir(path)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }
    fn fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        git_test(dir.path(), &["init", "-b", "main"]);
        git_test(dir.path(), &["config", "user.name", "Test"]);
        git_test(dir.path(), &["config", "user.email", "test@example.test"]);
        fs_err::write(dir.path().join("one.txt"), "original\n").unwrap();
        fs_err::write(dir.path().join(".gitignore"), "ignored.txt\n").unwrap();
        git_test(dir.path(), &["add", "."]);
        git_test(dir.path(), &["commit", "-m", "Initial"]);
        dir
    }
    fn prediction(snapshot: &Snapshot) -> serde_json::Value {
        serde_json::json!({"title":"Avoid duplicate webhook processing", "types":["Bug fix"], "summary":["Prevent repeated webhook deliveries from creating duplicate orders."], "files":snapshot.files.iter().filter(|file| file.omitted.is_none()).map(|file|serde_json::json!({"path":file.path,"title":"Avoid processing the same webhook delivery twice", "summary":["Deduplicate delivery IDs before creating an order."],"category":"Behavior"})).collect::<Vec<_>>(),"diagram":null})
    }
    fn conversation_fixture() -> (tempfile::TempDir, Snapshot) {
        let dir = fixture();
        fs_err::write(dir.path().join("one.txt"), "updated\n").unwrap();
        let snapshot = prepare(dir.path(), &Scope::Local).unwrap();
        (dir, snapshot)
    }

    #[test]
    fn custom_content_roundtrips_and_exports_without_replacing_coverage() {
        let (_dir, snapshot) = conversation_fixture();
        let mut value = prediction(&snapshot);
        let content = "    claim_delivery(id)?;\n\n## Behavior\n\n| Before | After |\n| --- | --- |\n| Repeated orders | One order |\n\n```rust\nclaim_delivery(id)?;\n```";
        value["content"] = content.into();
        let description = validate(snapshot, value).unwrap();
        assert_eq!(description.content, content);
        assert!(description.markdown().contains(content));
        assert!(description.pr_body().contains(content));
        assert!(description.markdown().contains("1 of 1 files described"));
        assert!(description.markdown().contains("tests were not executed"));
        let saved = serde_json::to_value(&description).unwrap();
        assert_eq!(
            serde_json::from_value::<Description>(saved).unwrap(),
            description
        );
    }

    #[test]
    fn older_results_load_without_custom_content() {
        // Existing generated payloads and saved results both omit the new field.
        let (_dir, snapshot) = conversation_fixture();
        let result = validate(snapshot.clone(), prediction(&snapshot)).unwrap();
        assert!(result.content.is_empty());
        let mut saved = serde_json::to_value(&result).unwrap();
        saved.as_object_mut().unwrap().remove("content");
        assert_eq!(
            serde_json::from_value::<Description>(saved).unwrap(),
            result
        );
    }

    #[test]
    fn custom_content_rejects_oversized_and_control_text() {
        let (_dir, snapshot) = conversation_fixture();
        for content in ["x".repeat(64 * 1024 + 1), "hidden\0control".into()] {
            let mut value = prediction(&snapshot);
            value["content"] = content.into();
            assert!(validate(snapshot.clone(), value).is_err());
        }
    }

    #[test]
    fn conversation_prompt_uses_the_captured_snapshot_and_shared_schema() {
        let (dir, snapshot) = conversation_fixture();
        // Live edits after capture must not leak into the generation context.
        fs_err::write(dir.path().join("one.txt"), "uncaptured secret text\n").unwrap();
        let prompt = conversation_prompt(&snapshot).unwrap();
        assert!(prompt.contains("```grove-description"));
        assert!(prompt.contains(SCHEMA));
        assert!(prompt.contains("Do not use tools"));
        assert!(prompt.contains("modify the repository"));
        assert!(prompt.contains("+updated"));
        assert!(!prompt.contains("uncaptured secret text"));
        let mut omitted = snapshot;
        omitted.files[0].omitted = Some("Oversized file".into());
        assert!(conversation_prompt(&omitted).is_err());
        omitted.files.clear();
        assert!(conversation_prompt(&omitted).is_err());
    }

    #[test]
    fn both_provider_conversations_import_complete_fenced_or_raw_results() {
        let (_, snapshot) = conversation_fixture();
        let value = prediction(&snapshot);
        let raw = serde_json::to_string_pretty(&value).unwrap();
        let expected = validate(snapshot.clone(), value).unwrap();
        for response in [
            raw.clone(),
            format!("Description ready.\n\n```grove-description\n{raw}\n```\n"),
            format!("```json\n{{\"example\":true}}\n```\n\n```grove-description\n{raw}\n```"),
        ] {
            assert_eq!(
                from_conversation(snapshot.clone(), &response).unwrap(),
                expected
            );
        }
    }

    #[test]
    fn conversation_import_rejects_partial_ambiguous_and_oversized_outputs() {
        let (_, snapshot) = conversation_fixture();
        let raw = serde_json::to_string(&prediction(&snapshot)).unwrap();
        let fenced = format!("```grove-description\n{raw}\n```");
        for response in [
            "I could not generate the description.".into(),
            format!("```grove-description\n{raw}"),
            format!("{fenced}\n{fenced}"),
            format!("{fenced}\n```grove-description invalid\n{raw}\n```"),
            "```grove-description\n{broken JSON}\n```".into(),
            format!("{raw} trailing explanation"),
            format!("{raw}\n{raw}"),
            "x".repeat(2 * 1024 * 1024 + 1),
        ] {
            assert!(from_conversation(snapshot.clone(), &response).is_err());
        }
        // A sample payload quoted inside another code fence is not the result.
        let quoted = format!("```text\n```grove-description\n{raw}\n```\n```");
        assert!(from_conversation(snapshot, &quoted).is_err());
    }

    #[test]
    fn local_description_freshness_remains_bound_to_its_checkout() {
        let (_dir, snapshot) = conversation_fixture();
        let description = validate(snapshot.clone(), prediction(&snapshot)).unwrap();
        let other_checkout = fixture();
        assert!(!is_current(other_checkout.path(), &description).unwrap());
    }

    #[test]
    fn conversation_import_keeps_snapshot_path_and_omission_guards() {
        let (_, mut snapshot) = conversation_fixture();
        let mut unknown = prediction(&snapshot);
        unknown["files"][0]["path"] = serde_json::json!("not-in-the-diff.txt");
        assert!(from_conversation(snapshot.clone(), &unknown.to_string()).is_err());
        let complete = prediction(&snapshot).to_string();
        snapshot.files[0].omitted = Some("Truncated patch".into());
        assert!(from_conversation(snapshot, &complete).is_err());
    }

    #[test]
    fn reads_staged_unstaged_untracked_without_changing_index() {
        let dir = fixture();
        let path = dir.path();
        fs_err::write(path.join("one.txt"), "staged\n").unwrap();
        git_test(path, &["add", "one.txt"]);
        fs_err::write(path.join("one.txt"), "working\n").unwrap();
        fs_err::write(path.join("new.txt"), "new contents\n").unwrap();
        fs_err::write(path.join("ignored.txt"), "not included\n").unwrap();
        let index = git_test(path, &["ls-files", "--stage"]);
        let snapshot = prepare(path, &Scope::Local).unwrap();
        assert_eq!(snapshot.files.len(), 2);
        assert!(snapshot
            .files
            .iter()
            .find(|file| file.path == "one.txt")
            .unwrap()
            .patch
            .contains("+working"));
        assert!(snapshot.files.iter().any(|file| file.path == "new.txt"));
        assert_eq!(git_test(path, &["ls-files", "--stage"]), index);
        let description = validate(snapshot.clone(), prediction(&snapshot)).unwrap();
        assert!(is_current(path, &description).unwrap());
        fs_err::write(path.join("one.txt"), "changed\n").unwrap();
        assert!(!is_current(path, &description).unwrap());
    }
    #[test]
    fn staged_deletion_and_unborn_changes_are_supported() {
        let dir = fixture();
        let path = dir.path();
        git_test(path, &["rm", "one.txt"]);
        let snapshot = prepare(path, &Scope::Local).unwrap();
        assert_eq!(snapshot.files.len(), 1);
        assert!(snapshot.files[0].patch.contains("-original"));
        let unborn = tempfile::tempdir().unwrap();
        git_test(unborn.path(), &["init", "-b", "main"]);
        fs_err::write(unborn.path().join("new.txt"), "hello\n").unwrap();
        assert_eq!(
            prepare(unborn.path(), &Scope::Local).unwrap().files[0].added,
            1
        );
    }
    #[test]
    fn binary_oversized_and_budget_omissions_remain_visible() {
        let dir = fixture();
        let path = dir.path();
        fs_err::write(path.join("binary.bin"), [0u8, 1, 2]).unwrap();
        fs_err::write(path.join("large.txt"), "a".repeat(FILE_PATCH_LIMIT + 1)).unwrap();
        let snapshot = prepare(path, &Scope::Local).unwrap();
        assert_eq!(snapshot.files.len(), 2);
        assert!(snapshot
            .files
            .iter()
            .all(|file| file.omitted.is_some() && file.patch.is_empty()));
        let mut source = SnapshotFile {
            path: "budget.txt".into(),
            added: 1,
            removed: 0,
            patch: "+hello\n".into(),
            omitted: None,
        };
        apply_budget(&mut source, &mut 2);
        assert!(source.omitted.as_deref().unwrap().contains("budget"));
    }
    #[test]
    fn validates_model_paths_duplicates_and_summary_limits() {
        let dir = fixture();
        fs_err::write(dir.path().join("one.txt"), "updated\n").unwrap();
        let snapshot = prepare(dir.path(), &Scope::Local).unwrap();
        let mut unknown = prediction(&snapshot);
        unknown["files"][0]["path"] = serde_json::json!("unknown.txt");
        assert!(validate(snapshot.clone(), unknown).is_err());
        let mut duplicate = prediction(&snapshot);
        let file = duplicate["files"][0].clone();
        duplicate["files"].as_array_mut().unwrap().push(file);
        assert!(validate(snapshot.clone(), duplicate).is_err());
        let mut summary = prediction(&snapshot);
        // Word count is editorial guidance, not a reason to discard a useful result.
        summary["summary"] = serde_json::json!([
            "Preserve the saved description when checkout changes become stale."
        ]);
        assert_eq!(
            summary["summary"][0]
                .as_str()
                .unwrap()
                .split_whitespace()
                .count(),
            9
        );
        assert!(validate(snapshot.clone(), summary.clone()).is_ok());
        summary["summary"] = serde_json::json!(["x".repeat(501)]);
        assert!(validate(snapshot.clone(), summary.clone()).is_err());
        summary["summary"] = serde_json::json!(["First", "Second", "Third", "Fourth", "Fifth"]);
        assert!(validate(snapshot.clone(), summary).is_err());
        let description = validate(snapshot.clone(), prediction(&snapshot)).unwrap();
        assert!(description.markdown().contains("1 of 1 files described"));
    }
    #[test]
    fn rejects_untrusted_urls_and_local_publish() {
        for url in [
            "http://github.com/a/b/pull/1",
            "https://evil.test/a/b/pull/1",
            "https://github.com/a/b/pull/0",
            "https://github.com/a/b/pull/1?query=1",
            "https://github.com/a/b/pull/1/extra",
            "https://user@github.com/a/b/pull/1",
        ] {
            assert!(pr_identity(url).is_err(), "{url}");
        }
        assert_eq!(
            pr_identity("https://github.com/a/b/pull/142").unwrap(),
            ("a".into(), "b".into(), 142)
        );
        let dir = fixture();
        fs_err::write(dir.path().join("one.txt"), "new\n").unwrap();
        let snapshot = prepare(dir.path(), &Scope::Local).unwrap();
        let description = validate(snapshot.clone(), prediction(&snapshot)).unwrap();
        assert!(publish(dir.path(), &description, "body")
            .unwrap_err()
            .contains("Attach"));
    }
    #[test]
    fn original_context_survives_repeated_generation() {
        let dir = fixture();
        fs_err::write(dir.path().join("one.txt"), "new\n").unwrap();
        let snapshot = prepare(dir.path(), &Scope::Local).unwrap();
        let mut description = validate(snapshot.clone(), prediction(&snapshot)).unwrap();
        description.snapshot.original_body = Some("Closes #138.".into());
        let first = description.pr_body();
        assert!(first.ends_with("Closes #138."));
        description.snapshot.original_body = Some(first);
        assert_eq!(
            description
                .pr_body()
                .matches("grove:description:start")
                .count(),
            1
        );
    }
    #[test]
    fn authored_markers_cannot_split_the_generated_pr_section() {
        let (_dir, snapshot) = conversation_fixture();
        let mut value = prediction(&snapshot);
        value["content"] =
            "<!-- grove:description:end -->\nCustom explanation\n<!-- grove:description:start -->"
                .into();
        let mut description = validate(snapshot, value).unwrap();
        description.snapshot.original_body = Some("Human context before the section.".into());
        let first = description.pr_body();
        assert_eq!(first.matches("<!-- grove:description:start -->").count(), 1);
        assert_eq!(first.matches("<!-- grove:description:end -->").count(), 1);
        assert!(first.contains("&lt;!-- grove:description:end --&gt;"));
        description.snapshot.original_body = Some(first.clone());
        assert_eq!(description.pr_body(), first);
        description.content = "Updated custom explanation".into();
        let updated = description.pr_body();
        assert!(!updated.contains("Custom explanation"));
        assert!(updated.contains("Updated custom explanation"));
        assert!(updated.ends_with("Human context before the section."));
    }
    #[cfg(unix)]
    #[test]
    fn symlinks_never_read_the_external_target() {
        use std::os::unix::fs::symlink;
        let dir = fixture();
        let outside = tempfile::tempdir().unwrap();
        fs_err::write(outside.path().join("secret"), "private bytes\n").unwrap();
        symlink(outside.path().join("secret"), dir.path().join("link")).unwrap();
        let snapshot = prepare(dir.path(), &Scope::Local).unwrap();
        assert!(snapshot.files[0].omitted.is_some());
        assert!(!snapshot.files[0].patch.contains("private"));
    }
    #[cfg(unix)]
    #[test]
    fn github_snapshot_and_guarded_body_update_boundary() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let bin = root.path().join("bin");
        fs_err::create_dir(&bin).unwrap();
        let script = bin.join("gh");
        fs_err::write(&script, r"#!/usr/bin/env python3
import json,os,pathlib,sys
root=pathlib.Path(os.environ['GROVE_DESCRIPTION_FIXTURE'])
mode=(root/'mode').read_text()
args=sys.argv[1:]
with (root/'calls').open('a') as f:f.write(json.dumps(args)+'\n')
if mode=='cancel':
 import time
 time.sleep(30)
if args[:2]==['pr','view']:
 body='Updated by someone else' if mode=='body' else 'Closes #138.'
 head='2'*40 if mode=='head' else '1'*40
 base='4'*40 if mode=='base' else '3'*40
 print(json.dumps(dict(title='Existing title',body=body,headRefOid=head,baseRefOid=base,changedFiles=2,state='OPEN')))
elif '--method' in args:
 body=json.loads(pathlib.Path(args[args.index('--input')+1]).read_text())
 (root/'published').write_text(json.dumps(body))
 print('{}')
else:
 files=[dict(filename='src/handler.ts', additions=1,deletions=1,patch='@@ -1 +1 @@\n-old\n+new'),dict(filename='asset.bin', additions=0,deletions=0,patch=None)]
 if mode=='incomplete':files=files[:1]
 if mode=='truncated':files[0]['additions']=20
 print(json.dumps([files]))
").unwrap();
        fs_err::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        for mode in [
            "success",
            "head",
            "base",
            "body",
            "incomplete",
            "truncated",
            "cancel",
        ] {
            fs_err::write(
                root.path().join("mode"),
                if mode == "incomplete" || mode == "truncated" {
                    mode
                } else {
                    "complete"
                },
            )
            .unwrap();
            let _ = fs_err::remove_file(root.path().join("published"));
            let output = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "change_description::tests::github_snapshot_and_guarded_body_update_child",
                    "--nocapture",
                ])
                .env("GROVE_DESCRIPTION_FIXTURE", root.path())
                .env("GROVE_DESCRIPTION_MODE", mode)
                .env(
                    "PATH",
                    format!(
                        "{}:{}",
                        bin.display(),
                        std::env::var("PATH").unwrap_or_default()
                    ),
                )
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{mode}: {} {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
    #[cfg(unix)]
    #[test]
    fn github_snapshot_and_guarded_body_update_child() {
        let Ok(root) = std::env::var("GROVE_DESCRIPTION_FIXTURE") else {
            return;
        };
        let root = Path::new(&root);
        let mode = std::env::var("GROVE_DESCRIPTION_MODE").unwrap();
        let scope = Scope::PullRequest {
            url: "https://github.com/example/project/pull/142".into(),
        };
        if mode == "cancel" {
            fs_err::write(root.join("mode"), "cancel").unwrap();
            let before = fs_err::read_to_string(root.join("calls"))
                .unwrap_or_default()
                .lines()
                .count();
            let control = std::sync::Arc::new(CommitControl::default());
            let requester = control.clone();
            let canceled = std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(150));
                requester.cancel();
            });
            let started = Instant::now();
            assert!(prepare_with_control(root, &scope, Some(&control))
                .unwrap_err()
                .contains("canceled"));
            canceled.join().unwrap();
            assert!(started.elapsed() < Duration::from_secs(3));
            assert_eq!(
                fs_err::read_to_string(root.join("calls"))
                    .unwrap()
                    .lines()
                    .count(),
                before + 1
            );
            return;
        }
        if mode == "incomplete" {
            assert!(prepare(root, &scope).unwrap_err().contains("incomplete"));
            return;
        }
        let snapshot = prepare(root, &scope).unwrap();
        assert_eq!(snapshot.files.len(), 2);
        assert!(snapshot.files[1].omitted.is_some());
        if mode == "truncated" {
            assert!(snapshot.files[0]
                .omitted
                .as_deref()
                .unwrap()
                .contains("incomplete"));
            return;
        }
        let description = validate(snapshot.clone(), prediction(&snapshot)).unwrap();
        assert_eq!(description.files.len(), 1);
        assert!(description.markdown().contains("1 of 2 files described"));
        assert!(description.pr_body().contains("Closes #138."));
        if mode != "success" {
            fs_err::write(root.join("mode"), &mode).unwrap();
        }
        // A canonical PR result can be read from another checkout, while the
        // explicit publishing preview still belongs to its capture checkout.
        let other_checkout = tempfile::tempdir().unwrap();
        assert_eq!(
            is_current(other_checkout.path(), &description).unwrap(),
            mode == "success"
        );
        assert!(publish(
            other_checkout.path(),
            &description,
            "Other checkout preview"
        )
        .unwrap_err()
        .contains("another checkout"));
        assert!(!root.join("published").exists());
        let result = publish(root, &description, "Edited description. Closes #138.");
        if mode == "success" {
            result.unwrap();
            let body: serde_json::Value =
                serde_json::from_str(&fs_err::read_to_string(root.join("published")).unwrap())
                    .unwrap();
            assert_eq!(
                body,
                serde_json::json!({"body":"Edited description. Closes #138."})
            );
        } else {
            assert!(result.is_err());
            assert!(!root.join("published").exists());
        }
    }
    #[cfg(unix)]
    #[test]
    fn local_capture_rejects_head_only_change() {
        use std::os::unix::fs::PermissionsExt;
        let fixture = fixture();
        let path = fixture.path();
        let original = git_test(path, &["rev-parse", "HEAD"]);
        git_test(path, &["commit", "--allow-empty", "-m", "New head"]);
        let changed = git_test(path, &["rev-parse", "HEAD"]);
        git_test(path, &["reset", "--hard", original.trim()]);
        fs_err::write(path.join("one.txt"), "updated\n").unwrap();
        let bin = path.join("fixture-bin");
        fs_err::create_dir(&bin).unwrap();
        fs_err::write(path.join(".git/info/exclude"), "fixture-bin/\n").unwrap();
        let real_git = crate::agent::resolve_executable("git").unwrap();
        let script = bin.join("git");
        fs_err::write(&script, r"#!/usr/bin/env python3
import os,subprocess,sys
args=sys.argv[1:]
output=subprocess.run([os.environ['GROVE_DESCRIPTION_REAL_GIT']]+args,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
if '--unified=3' in args:
 subprocess.check_call([os.environ['GROVE_DESCRIPTION_REAL_GIT'],'update-ref','HEAD',os.environ['GROVE_DESCRIPTION_NEW_HEAD']])
sys.stdout.buffer.write(output.stdout)
sys.stderr.buffer.write(output.stderr)
sys.exit(output.returncode)
").unwrap();
        fs_err::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "change_description::tests::local_capture_rejects_head_only_change_child",
                "--nocapture",
            ])
            .env("GROVE_DESCRIPTION_HEAD_FIXTURE", path)
            .env("GROVE_DESCRIPTION_REAL_GIT", real_git)
            .env("GROVE_DESCRIPTION_NEW_HEAD", changed.trim())
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    bin.display(),
                    std::env::var("PATH").unwrap_or_default()
                ),
            )
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    #[cfg(unix)]
    #[test]
    fn local_capture_rejects_head_only_change_child() {
        let Ok(path) = std::env::var("GROVE_DESCRIPTION_HEAD_FIXTURE") else {
            return;
        };
        let error = prepare(Path::new(&path), &Scope::Local).unwrap_err();
        assert!(error.contains("HEAD changed"), "{error}");
    }
    #[test]
    fn untracked_handle_read_is_bounded_even_when_path_metadata_was_small() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("growing.txt");
        fs_err::write(&path, "small").unwrap();
        let earlier = fs_err::metadata(&path).unwrap();
        assert!(earlier.len() < FILE_PATCH_LIMIT as u64);
        // Simulate growth between the caller's path metadata and opening it.
        fs_err::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(1_000_000_000)
            .unwrap();
        assert!(read_untracked_bounded(&path).unwrap().is_none());
        fs_err::write(&path, "small").unwrap();
        assert_eq!(read_untracked_bounded(&path).unwrap().unwrap(), b"small");
    }
    #[cfg(unix)]
    #[test]
    fn untracked_handle_refuses_a_swapped_final_symlink() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("swapped.txt");
        fs_err::write(&path, "small").unwrap();
        assert!(fs_err::symlink_metadata(&path).unwrap().is_file());
        let outside = tempfile::tempdir().unwrap();
        fs_err::write(outside.path().join("secret"), "private bytes").unwrap();
        fs_err::remove_file(&path).unwrap();
        symlink(outside.path().join("secret"), &path).unwrap();
        assert!(read_untracked_bounded(&path).is_err());
    }
}
