//! Manual worktree form: suggestions stop permanently at the first branch edit.
mod settings;
pub(in crate::views) use settings::{WorktreeSetup, WorktreeSetupEvent};
use std::process::Command;

fn suggested_branch(name: &str) -> String {
    let slug = name
        .trim()
        .to_ascii_lowercase()
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if slug.is_empty() {
        String::new()
    } else {
        format!("feat/{slug}")
    }
}

fn resolved_source(project: &str, revision: &str) -> Option<String> {
    let output = Command::new("git")
        .args([
            "-C",
            project,
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{revision}^{{commit}}"),
        ])
        .output();
    let sha = output.ok().filter(|out| out.status.success()).map(|out| {
        String::from_utf8_lossy(&out.stdout)
            .trim()
            .chars()
            .take(7)
            .collect::<String>()
    })?;
    let source = if revision == "HEAD" {
        Command::new("git")
            .args(["-C", project, "symbolic-ref", "--quiet", "--short", "HEAD"])
            .output()
            .ok()
            .filter(|out| out.status.success())
            .map_or_else(
                || "Detached HEAD".into(),
                |out| String::from_utf8_lossy(&out.stdout).trim().to_string(),
            )
    } else {
        revision.to_owned()
    };
    Some(format!("{source} · {sha}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn branch_suggestion_is_a_valid_git_name() {
        for (name, expected) in [
            ("billing-retry", "feat/billing-retry"),
            (" Billing retry ", "feat/billing-retry"),
            ("foo..bar/@{bad}.lock", "feat/foo-bar-bad-lock"),
            ("", ""),
            ("///", ""),
        ] {
            assert_eq!(suggested_branch(name), expected);
        }
    }
    #[test]
    fn source_resolves_actual_head_detached_and_invalid_revision() {
        let repo = tempfile::tempdir().unwrap();
        for args in [
            vec!["init", "-qb", "actual-source"],
            vec![
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "commit",
                "--allow-empty",
                "-qm",
                "Fixture",
            ],
        ] {
            assert!(Command::new("git")
                .current_dir(repo.path())
                .args(args)
                .status()
                .unwrap()
                .success());
        }
        let path = repo.path().to_str().unwrap();
        assert!(resolved_source(path, "HEAD").unwrap().starts_with("actual-source · "));
        assert!(resolved_source(path, "not-a-revision").is_none());
        assert!(Command::new("git")
            .current_dir(repo.path())
            .args(["checkout", "--detach", "-q"])
            .status()
            .unwrap()
            .success());
        assert!(resolved_source(path, "HEAD").unwrap().starts_with("Detached HEAD · "));
    }
}
