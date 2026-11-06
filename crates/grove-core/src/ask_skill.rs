//! Grove-owned read-only questions, intercepted before either provider receives a command.
use crate::describe_skill::ScopePreference;
pub const NAME: &str = "ask";
pub const USAGE: &str = "Use /ask [local|pr|PR-URL] [question] or $ask [local|pr|PR-URL] [question]. Omit the question to open the question reader.";
pub const SKILL: &str = include_str!("../../../product/skills/ask/SKILL.md");
pub const GUIDANCE: &str = "Grove's /ask or $ask command opens a native read-only question reader about local changes or an attached PR. Optional local, pr or a GitHub PR URL selects scope; remaining text is the question. Grove intercepts before provider submission and starts a dedicated retained conversation with the selected provider/model preferences. The native answer is validated and saved per source with its exact question. Conversation opens the same session; ordinary follow-ups do not replace the saved answer. Existing provider permissions apply, with an initial read-only request. Mentioning the command does not run it.";
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Invocation {
    pub scope: ScopePreference,
    pub question: String,
}
pub fn is_invocation(text: &str) -> bool {
    matches!(text.split_whitespace().next(), Some("/ask" | "$ask"))
}
pub fn invocation(text: &str) -> Option<Invocation> {
    let text = text.trim();
    if !is_invocation(text) {
        return None;
    }
    let (_, rest) = text.split_once(char::is_whitespace).unwrap_or((text, ""));
    let rest = rest.trim();
    let (first, tail) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
    let (scope, question) = match first {
        "local" => (ScopePreference::Local, tail.trim()),
        "pr" => (ScopePreference::PullRequest, tail.trim()),
        url if crate::change_description::pr_identity(url).is_ok() => {
            (ScopePreference::PullRequestUrl(url.into()), tail.trim())
        }
        url if url.contains("://") || url.starts_with("github.com/") => return None,
        _ => (ScopePreference::Current, rest),
    };
    if question.len() > 4000 || question.starts_with("--") {
        return None;
    }
    Some(Invocation {
        scope,
        question: question.into(),
    })
}
pub fn completion(prefix: char, query: &str) -> Option<(String, String)> {
    (matches!(prefix, '/' | '$') && NAME.starts_with(query)).then(|| {
        let command = format!("{prefix}{NAME}");
        (
            command.clone(),
            format!("{command} · Ask about local changes or the attached PR"),
        )
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_question_without_changing_bytes_or_swallowing_prose() {
        for prefix in ["/ask", "$ask"] {
            assert_eq!(
                invocation(&format!("{prefix} pr Why does this fail?\nShow examples.")).unwrap(),
                Invocation {
                    scope: ScopePreference::PullRequest,
                    question: "Why does this fail?\nShow examples.".into()
                }
            );
            assert_eq!(invocation(prefix).unwrap().question, "");
            assert_eq!(
                invocation(&format!("{prefix} Why local caching? "))
                    .unwrap()
                    .question,
                "Why local caching?"
            );
            assert!(is_invocation(&format!("{prefix} --publish")));
            assert!(invocation(&format!("{prefix} --publish")).is_none());
        }
        for text in ["Explain /ask", "`/ask`", "/asking", "$asking", ""] {
            assert!(!is_invocation(text));
        }
    }
    #[test]
    fn reserves_both_catalog_forms() {
        assert_eq!(completion('/', "as").unwrap().0, "/ask");
        assert_eq!(completion('$', "ask").unwrap().0, "$ask");
        assert!(completion('@', "ask").is_none());
    }
}
