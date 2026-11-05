//! The bundled, user-invoked Grove skill shared by Codex and Claude.
//!
//! Invocation is owned by Grove's composer, rather than submitted as an unknown
//! provider slash command. The native action uses the same snapshot/generator as
//! Checkout. No provider-global skill directory or configuration is modified.

pub const NAME: &str = "review";
pub const COMMAND: &str = "/review";
pub const DESCRIPTION: &str = "Review local changes or the attached PR in Grove";
pub const USAGE: &str = "Use /review [local|pr] or $review [local|pr].";
pub const SKILL: &str = include_str!("../../../product/skills/review/SKILL.md");
pub const GUIDANCE: &str = "Grove includes a built-in review skill for Codex and Claude. The user can invoke /review or $review, optionally followed by local or pr, in Grove's composer, or choose Review changes in Checkout. Grove handles this native command before provider submission, generates snapshot-bound actionable findings and coverage, and opens its native reader. It does not change the main conversation or automatically publish a PR review. Mentioning the command in an assistant response does not invoke it; do not claim generation or publishing without an observed Grove outcome. This skill adds no agent tool or permissions.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScopePreference {
    Current,
    Local,
    PullRequest,
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
    fn unknown_arguments_are_recognized_but_not_executed() {
        for text in [
            "/review --publish",
            "$review local pr",
            "/review https://github.com/org/repo/pull/142",
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
        assert!(SKILL.contains("Read-only side conversations do not receive"));
    }
}
