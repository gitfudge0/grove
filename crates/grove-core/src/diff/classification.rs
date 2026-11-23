//! Checkout change categories and aggregated file counts.

use super::{changed_files, FileChange};
use fs_err as fs;
use std::path::Path;

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

    pub(super) fn from_json(bytes: &[u8]) -> Result<Self, String> {
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
