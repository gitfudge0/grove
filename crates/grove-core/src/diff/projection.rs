//! Row projections for change lists, file trees and changed word spans.

use super::FileChange;
use std::collections::HashSet;

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
