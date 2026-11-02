//! Capability gate for tool-free Claude side conversations.
//!
//! `--safe-mode` and `disableAllHooks` retain administrator-managed hooks.
//! Only `--bare` skips configured hooks without discovering managed policy.
//! Bare mode deliberately does not use subscription OAuth or the keychain:
//! require an already-exported API key instead of weakening that boundary.
//! See https://code.claude.com/docs/en/headless#start-faster-with-bare-mode.

const UNAVAILABLE: &str = "Claude Side chat requires an exported ANTHROPIC_API_KEY for isolated Haiku requests. Subscription sign-in alone is unavailable because Claude's OAuth mode can run managed hooks.";

/// Checks the same exported environment used for provider launches, without
/// reading credential files, consulting the keychain, or launching Claude.
/// Callers must still launch Claude with `--bare`, no tools, and strict MCP.
pub fn ensure_read_only() -> Result<(), String> {
    let available = if let Some(environment) = crate::env_path::session_environment() {
        has_api_key(
            environment
                .iter()
                .map(|(key, value)| (key.as_str(), value.as_str())),
        )
    } else {
        // A missing captured shell environment is not evidence of credentials
        // that a later interactive shell might discover. Fail closed unless
        // the key is already exported into this process.
        std::env::var("ANTHROPIC_API_KEY").is_ok_and(|value| !value.trim().is_empty())
    };
    if available {
        Ok(())
    } else {
        Err(UNAVAILABLE.into())
    }
}

fn has_api_key<'a>(environment: impl IntoIterator<Item = (&'a str, &'a str)>) -> bool {
    environment
        .into_iter()
        .any(|(key, value)| key == "ANTHROPIC_API_KEY" && !value.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subscription_and_helper_credentials_do_not_authorize_bare_requests() {
        assert!(!has_api_key([
            ("CLAUDE_CODE_OAUTH_TOKEN", "subscription-token"),
            ("ANTHROPIC_AUTH_TOKEN", "proxy-token"),
            ("ANTHROPIC_API_KEY", "  "),
        ]));
        assert!(!has_api_key(std::iter::empty()));
        assert!(has_api_key([("ANTHROPIC_API_KEY", "exported-key")]));
    }
}
