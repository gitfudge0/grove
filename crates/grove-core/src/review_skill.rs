//! The bundled, user-invoked Grove skill shared by Codex and Claude.
//!
//! Invocation is owned by Grove's composer, rather than submitted as an unknown
//! provider slash command. The native action uses the same snapshot/generator as
//! Checkout. No provider-global skill directory or configuration is modified.

pub const NAME: &str = "review";
pub const COMMAND: &str = "/review";
pub const DESCRIPTION: &str = "Review local changes or the attached PR in Grove";
pub const USAGE: &str = "Use /review [local|pr|PR-URL] or $review [local|pr|PR-URL].";
pub const SKILL: &str = include_str!("../../../product/skills/review/SKILL.md");
pub const GUIDANCE: &str = "Grove includes a built-in review skill for Codex and Claude. The user can invoke /review or $review, optionally followed by local, pr, or a GitHub PR URL, in Grove's composer, or choose Review changes in Checkout. Grove handles this native command before provider submission, creates a dedicated retained conversation and saves snapshot-bound actionable findings and coverage, and opens its native reader. The originating conversation and draft stay intact. View review reopens the saved result after restart; Conversation opens the same dedicated provider session. Initial generation requests read-only behavior but uses the session permissions. No review is automatically published. Mentioning the command in an assistant response does not invoke it; do not claim generation or publishing without an observed Grove outcome. This skill adds no agent tool or permissions.";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScopePreference {
    Current,
    Local,
    PullRequest,
    PullRequestUrl(String),
}

/// Recognize only a complete user command, not embedded prose or selections.
pub fn invocation(text: &str) -> Option<ScopePreference> {
    let mut words = text.split_whitespace();
    if !matches!(words.next(), Some("/review" | "$review")) {
        return None;
    }
    let scope = match words.next() {
        None => ScopePreference::Current,
        Some("local") => ScopePreference::Local,
        Some("pr") => ScopePreference::PullRequest,
        Some(url) if crate::change_description::pr_identity(url).is_ok() => {
            ScopePreference::PullRequestUrl(url.into())
        }
        _ => return None,
    };
    words.next().is_none().then_some(scope)
}

/// A known command with invalid arguments must show usage instead of escaping
/// into the provider's own command parser.
pub fn is_invocation(text: &str) -> bool {
    matches!(text.split_whitespace().next(), Some("/review" | "$review"))
}

/// Share one completion across slash commands and Codex-style skill mentions.
/// The host reserves this name to avoid duplicate provider catalog entries.
pub fn completion(prefix: char, query: &str) -> Option<(String, String)> {
    (matches!(prefix, '/' | '$') && NAME.starts_with(query)).then(|| {
        let command = format!("{prefix}{NAME}");
        (command.clone(), format!("{command} · {DESCRIPTION}"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_provider_spellings_share_the_scope_contract() {
        for command in ["/review", "$review"] {
            assert_eq!(invocation(command), Some(ScopePreference::Current));
            assert_eq!(
                invocation(&format!("  {command} local \n")),
                Some(ScopePreference::Local)
            );
            assert_eq!(
                invocation(&format!("{command}\tpr")),
                Some(ScopePreference::PullRequest)
            );
        }
    }

    #[test]
    fn explicit_pr_url_is_validated_without_losing_its_source() {
        assert_eq!(
            invocation("/review https://github.com/Org/Repo/pull/142"),
            Some(ScopePreference::PullRequestUrl(
                "https://github.com/Org/Repo/pull/142".into()
            ))
        );
        assert!(invocation("/review https://evil.test/Org/Repo/pull/142").is_none());
    }
    #[test]
    fn unknown_arguments_are_recognized_but_not_executed() {
        for text in [
            "/review --publish",
            "$review local pr",
            "/review https://github.com/org/repo/issues/142",
        ] {
            assert!(is_invocation(text));
            assert_eq!(invocation(text), None);
        }
    }

    #[test]
    fn prose_and_quoted_commands_do_not_trigger_native_work() {
        for text in [
            "Explain /review",
            "`/review`",
            "```\n/review\n```",
            "Referenced transcript selection (quoted data):\n/review\n\nUser reply:\nExplain it",
            "/review-other",
            "$review-other",
            "",
        ] {
            assert!(!is_invocation(text));
            assert_eq!(invocation(text), None);
        }
    }

    #[test]
    fn completion_has_no_provider_or_permission_dependency() {
        assert_eq!(completion('/', "review").unwrap().0, COMMAND);
        assert_eq!(completion('$', "review").unwrap().0, "$review");
        assert!(completion('/', "not-a-skill").is_none());
        assert!(completion('@', "review").is_none());
        assert!(SKILL.contains("name: review"));
        assert!(SKILL
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .contains("Read-only side conversations do not receive"));
    }
}
