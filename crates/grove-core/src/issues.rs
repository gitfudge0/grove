//! Read-only issue lookup. Credentials live exclusively in the OS credential store.
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fmt::Write as _;
use std::{path::Path, time::Duration};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IssueReference {
    Github {
        owner: String,
        repo: String,
        number: u64,
    },
    Jira {
        origin: String,
        key: String,
    },
    Linear {
        key: String,
    },
    Key(String),
}
fn issue_key(s: &str) -> bool {
    s.len() <= 128
        && s.rsplit_once('-').is_some_and(|(prefix, n)| {
            !prefix.is_empty()
                && prefix
                    .bytes()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
                && !n.is_empty()
                && n.bytes().all(|c| c.is_ascii_digit())
        })
}
pub fn parse_reference(input: &str) -> Result<IssueReference, String> {
    let input = input.trim();
    if input.len() > 4096 {
        return Err("Issue reference is too long.".into());
    }
    if issue_key(input) {
        return Ok(IssueReference::Key(input.into()));
    }
    let u = url::Url::parse(input)
        .map_err(|_| "Paste a GitHub, Jira Cloud or Linear issue URL, or an issue key.")?;
    if u.scheme() != "https"
        || !u.username().is_empty()
        || u.password().is_some()
        || u.port().is_some()
        || u.query().is_some()
        || u.fragment().is_some()
    {
        return Err("Use an HTTPS issue URL without credentials, port, query or fragment.".into());
    }
    let host = u.host_str().unwrap_or_default();
    let parts: Vec<_> = u.path().trim_matches('/').split('/').collect();
    match host {
        "github.com"
            if parts.len() == 4
                && parts[2] == "issues"
                && parts[..2].iter().all(|s| {
                    !s.is_empty()
                        && s.bytes()
                            .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
                }) =>
        {
            let number = parts[3]
                .parse::<u64>()
                .ok()
                .filter(|n| *n > 0)
                .ok_or("Invalid GitHub issue number.")?;
            Ok(IssueReference::Github {
                owner: parts[0].into(),
                repo: parts[1].into(),
                number,
            })
        }
        "linear.app" if parts.len() >= 3 && parts[1] == "issue" && issue_key(parts[2]) => {
            Ok(IssueReference::Linear {
                key: parts[2].into(),
            })
        }
        _ if host.ends_with(".atlassian.net")
            && parts.len() == 2
            && parts[0] == "browse"
            && issue_key(parts[1]) =>
        {
            Ok(IssueReference::Jira {
                origin: format!("https://{host}"),
                key: parts[1].into(),
            })
        }
        _ => Err(
            "Unsupported issue URL. Use github.com/issues, linear.app or a Jira Cloud browse URL."
                .into(),
        ),
    }
}
#[derive(Clone, Debug)]
pub struct ResolvedIssue {
    pub provider: &'static str,
    pub immutable_id: String,
    pub id: String,
    pub url: String,
    pub title: String,
    pub description: String,
}
impl ResolvedIssue {
    pub fn suggested_branch(&self) -> String {
        let slug = self
            .title
            .to_ascii_lowercase()
            .split(|c: char| !c.is_ascii_alphanumeric())
            .filter(|s| !s.is_empty())
            .take(8)
            .collect::<Vec<_>>()
            .join("-");
        let slug = slug.chars().take(64).collect::<String>();
        format!(
            "{}{}{}",
            self.id.to_ascii_lowercase(),
            if slug.is_empty() { "" } else { "-" },
            slug
        )
    }
}
#[derive(Default, Serialize, Deserialize)]
pub struct Connections {
    pub linear_token: String,
    pub jira_origin: String,
    pub jira_email: String,
    pub jira_token: String,
}
fn credential(project: &str) -> Result<keyring::Entry, String> {
    // Digest avoids exposing checkout names in credential metadata.
    let digest = ring::digest::digest(&ring::digest::SHA256, project.as_bytes());
    let mut account = String::with_capacity(64);
    for byte in digest.as_ref() {
        let _ = write!(account, "{byte:02x}");
    }
    keyring::Entry::new("Grove issue connections", &account)
        .map_err(|_| "OS credential storage is unavailable.".into())
}
pub fn load_connections(project: &str) -> Result<Connections, String> {
    match credential(project)?.get_password() {
        Ok(s) => serde_json::from_str(&s).map_err(|_| {
            "Saved issue connection could not be read. Save the connection again.".into()
        }),
        Err(keyring::Error::NoEntry) => Ok(Connections::default()),
        Err(_) => Err("Unlock your OS credential store to access issue connections.".into()),
    }
}
pub fn save_connection(
    project: &str,
    provider: &str,
    origin: &str,
    email: &str,
    token: &str,
) -> Result<(), String> {
    if token.trim().is_empty() {
        return Err("Enter an API token.".into());
    }
    let mut c = load_connections(project)?;
    match provider {
        "Linear" => c.linear_token = token.trim().into(),
        "Jira" => {
            let IssueReference::Jira { origin, .. } = parse_reference(&format!(
                "{}/browse/GROVE-1",
                origin.trim().trim_end_matches('/')
            ))?
            else {
                return Err("Enter a Jira Cloud site URL.".into());
            };
            if !email.contains('@') {
                return Err("Enter your Atlassian account email.".into());
            }
            c.jira_origin = origin;
            c.jira_email = email.trim().into();
            c.jira_token = token.trim().into();
        }
        _ => return Err("Choose Jira or Linear.".into()),
    }
    credential(project)?
        .set_password(&serde_json::to_string(&c).map_err(|_| "Could not encode connection.")?)
        .map_err(|_| "Could not save connection in your OS credential store.".into())
}
fn api_error(e: ureq::Error) -> String {
    match e {
        ureq::Error::StatusCode(401 | 403) => {
            "Issue access denied. Repair the provider connection and retry.".into()
        }
        ureq::Error::StatusCode(404) => "Issue not found or inaccessible with this account.".into(),
        _ => "Issue lookup failed. Check your connection and retry.".into(),
    }
}
fn text(v: &Value) -> String {
    if let Some(s) = v.as_str() {
        return s.into();
    }
    let mut result = String::new();
    if let Some(s) = v["text"].as_str() {
        result.push_str(s);
    }
    if let Some(children) = v["content"].as_array() {
        for child in children {
            result.push_str(&text(child));
            result.push('\n');
        }
    }
    result
}
fn configured_reference(r: IssueReference, c: &Connections) -> Result<IssueReference, String> {
    if let IssueReference::Key(key) = r {
        return match (!c.linear_token.is_empty(), !c.jira_token.is_empty()) {
            (true, false) => Ok(IssueReference::Linear { key }),
            (false, true) => Ok(IssueReference::Jira {
                origin: c.jira_origin.clone(),
                key,
            }),
            (true, true) => {
                return Err("This key is ambiguous: paste a Jira or Linear issue URL.".into());
            }
            _ => return Err("Connect Jira or Linear before resolving a bare issue key.".into()),
        };
    }
    Ok(r)
}
pub fn resolve(project: &str, reference: &str) -> Result<ResolvedIssue, String> {
    let mut r = parse_reference(reference)?;
    let c = if matches!(r, IssueReference::Github { .. }) {
        Connections::default()
    } else {
        load_connections(project)?
    };
    r = configured_reference(r, &c)?;
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(20)))
        .max_redirects(0)
        .build()
        .into();
    match r {
        IssueReference::Github {
            owner,
            repo,
            number,
        } => {
            let output = github_issue(
                Path::new(project),
                &format!("https://github.com/{owner}/{repo}"),
                number,
            )?;
            let v: Value = serde_json::from_str(&output)
                .map_err(|_| "GitHub returned an invalid issue response.")?;
            Ok(ResolvedIssue {
                provider: "GitHub",
                immutable_id: text(&v["id"]),
                id: format!("issue-{number}"),
                url: format!("https://github.com/{owner}/{repo}/issues/{number}"),
                title: v["title"].as_str().unwrap_or_default().into(),
                description: text(&v["body"]),
            })
        }
        IssueReference::Linear { key } => {
            if c.linear_token.is_empty() {
                return Err("Connect Linear with a personal API key, then retry.".into());
            }
            let body = json!({"query":"query GroveIssue($id: String!) { issue(id: $id) { id identifier title description url } }","variables":{"id":key}});
            let v: Value = agent
                .post("https://api.linear.app/graphql")
                .header("Authorization", &c.linear_token)
                .send_json(body)
                .map_err(api_error)?
                .body_mut()
                .read_json()
                .map_err(|_| "Linear returned an invalid response.")?;
            let issue = &v["data"]["issue"];
            if !v["errors"].is_null() || issue.is_null() {
                return Err(
                    "Linear issue not found or access denied. Check the key and connection.".into(),
                );
            }
            Ok(ResolvedIssue {
                provider: "Linear",
                immutable_id: text(&issue["id"]),
                id: issue["identifier"].as_str().unwrap_or(&key).into(),
                url: issue["url"].as_str().unwrap_or_default().into(),
                title: text(&issue["title"]),
                description: text(&issue["description"]),
            })
        }
        IssueReference::Jira { origin, key } => {
            if c.jira_token.is_empty() || c.jira_origin != origin {
                return Err(
                    "Connect this Jira Cloud site with your email and API token, then retry."
                        .into(),
                );
            }
            let auth = base64::engine::general_purpose::STANDARD
                .encode(format!("{}:{}", c.jira_email, c.jira_token));
            let v: Value = agent
                .get(&format!(
                    "{origin}/rest/api/3/issue/{key}?fields=summary,description"
                ))
                .header("Authorization", format!("Basic {auth}"))
                .call()
                .map_err(api_error)?
                .body_mut()
                .read_json()
                .map_err(|_| "Jira returned an invalid response.")?;
            Ok(ResolvedIssue {
                provider: "Jira",
                immutable_id: text(&v["id"]),
                id: key.clone(),
                url: format!("{origin}/browse/{key}"),
                title: text(&v["fields"]["summary"]),
                description: text(&v["fields"]["description"]),
            })
        }
        IssueReference::Key(_) => unreachable!(),
    }
}
fn github_issue(project: &Path, repo: &str, number: u64) -> Result<String, String> {
    use std::{
        io::{Read, Seek, SeekFrom},
        process::{Command, Stdio},
        time::Instant,
    };
    let mut out = tempfile::tempfile().map_err(|_| "Could not prepare GitHub lookup.")?;
    let mut child = Command::new("gh")
        .current_dir(project)
        .args([
            "issue",
            "view",
            &number.to_string(),
            "--repo",
            repo,
            "--json",
            "id,title,body",
        ])
        .env("GH_PROMPT_DISABLED", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::from(
            out.try_clone()
                .map_err(|_| "Could not prepare GitHub lookup.")?,
        ))
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "GitHub CLI is unavailable. Install gh and sign in with gh auth login.")?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    return Err(
                        "GitHub issue lookup failed. Run gh auth login and check issue access."
                            .into(),
                    );
                }
                break;
            }
            Ok(None) if start.elapsed() < Duration::from_secs(20) => {
                std::thread::sleep(Duration::from_millis(25));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(
                    "GitHub issue lookup timed out. Retry after checking connectivity.".into(),
                );
            }
        }
    }
    out.seek(SeekFrom::Start(0))
        .map_err(|_| "Could not read GitHub issue.")?;
    let mut result = String::new();
    out.take(1_048_577)
        .read_to_string(&mut result)
        .map_err(|_| "Could not read GitHub issue.")?;
    if result.len() > 1_048_576 {
        return Err("Issue response exceeds the size limit.".into());
    }
    Ok(result)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bare_keys_require_one_configured_provider_and_recover_after_connection() {
        let key = || IssueReference::Key("ABC-1".into());
        let mut c = Connections::default();
        assert!(configured_reference(key(), &c)
            .unwrap_err()
            .contains("Connect"));
        c.linear_token = "test".into();
        assert!(matches!(
            configured_reference(key(), &c),
            Ok(IssueReference::Linear { .. })
        ));
        c.jira_token = "test".into();
        c.jira_origin = "https://a.atlassian.net".into();
        assert!(configured_reference(key(), &c)
            .unwrap_err()
            .contains("ambiguous"));
        c.linear_token.clear();
        assert!(
            matches!(configured_reference(key(),&c),Ok(IssueReference::Jira{origin,..}) if origin==c.jira_origin)
        );
    }
    #[test]
    fn references_bind_credentials_to_safe_origins() {
        assert!(matches!(
            parse_reference("https://a.atlassian.net/browse/ABC-12"),
            Ok(IssueReference::Jira { .. })
        ));
        assert!(matches!(
            parse_reference("https://linear.app/team/issue/ABC-12/title"),
            Ok(IssueReference::Linear { .. })
        ));
        assert!(matches!(
            parse_reference("https://github.com/org/repo/issues/12"),
            Ok(IssueReference::Github { number: 12, .. })
        ));
        assert!(matches!(
            parse_reference("ABC-12"),
            Ok(IssueReference::Key(_))
        ));
        for bad in [
            "https://evil.test/browse/ABC-1",
            "http://a.atlassian.net/browse/ABC-1",
            "https://x:y@a.atlassian.net/browse/ABC-1",
            "https://a.atlassian.net:444/browse/ABC-1",
            "https://github.com/o/r/issues/0",
            "https://github.com/o/r/issues/1?token=x",
        ] {
            assert!(parse_reference(bad).is_err(), "{bad}");
        }
    }
    #[test]
    fn suggestion_is_valid_and_issue_text_is_data() {
        let i = ResolvedIssue {
            provider: "Jira",
            immutable_id: "10001".into(),
            id: "ABC-1".into(),
            url: String::new(),
            title: "Fix `$(rm -rf)` / build!!!".into(),
            description: String::new(),
        };
        assert!(crate::git::valid_worktree_name(&i.suggested_branch()));
        assert_eq!(i.suggested_branch(), "abc-1-fix-rm-rf-build");
    }
    #[test]
    fn jira_document_description() {
        assert_eq!(text(&json!({"content":[{"text":"Hello"}]})), "Hello\n");
    }
}
