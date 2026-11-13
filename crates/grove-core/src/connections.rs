//! Provider-owned credentials, with bounded discovery and nonpersistent sign-in jobs.
//!
//! Nothing in this module is serialized or logged. Authentication output, links,
//! codes and interactive input live only for the lifetime of an explicit job.
use crate::{agent::Agent, provider::Provider};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use serde_json::Value;
use std::{
    collections::HashSet,
    io::{BufRead, BufReader, Read, Write},
    path::PathBuf,
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, OnceLock,
    },
    time::{Duration, Instant},
};

const QUERY_TIMEOUT: Duration = Duration::from_secs(20);
const LOGIN_TIMEOUT: Duration = Duration::from_mins(10);
const OUTPUT_LIMIT: usize = 64 * 1024;

#[derive(Clone)]
pub struct ConnectionContext {
    pub provider: Provider,
    pub cwd: PathBuf,
    pub args: Vec<String>,
    environment: Vec<(String, String)>,
    program: PathBuf,
    identity: String,
    generation: Vec<u8>,
}
impl ConnectionContext {
    pub fn new(
        provider: Provider,
        cwd: impl Into<PathBuf>,
        args: Vec<String>,
    ) -> Result<Self, String> {
        let cwd = cwd.into();
        let cwd = fs_err::canonicalize(cwd)
            .map_err(|_| "The selected checkout is unavailable".to_owned())?;
        let name = provider.agent().label();
        let program = crate::agent::resolve_executable(name).ok_or_else(|| {
            format!(
                "{} is not installed. Open Provider versions for tooling information.",
                provider.label()
            )
        })?;
        let environment = crate::env_path::session_environment()
            .map_or_else(|| std::env::vars().collect(), |env| env.to_vec());
        let identity = credential_key(provider, &environment);
        let mut context = Self {
            provider,
            cwd,
            args,
            environment,
            program,
            identity,
            generation: Vec::new(),
        };
        context.generation = context.configuration_generation().unwrap_or_default();
        Ok(context)
    }
    pub fn externally_managed(&self) -> bool {
        let secrets: &[&str] = match self.provider {
            Provider::Codex => &["OPENAI_API_KEY", "CODEX_ACCESS_TOKEN"],
            Provider::Claude => &[
                "ANTHROPIC_API_KEY",
                "ANTHROPIC_AUTH_TOKEN",
                "CLAUDE_CODE_OAUTH_TOKEN",
            ],
        };
        self.environment.iter().any(|(key, value)| {
            secrets.contains(&key.as_str()) && !value.is_empty()
                || self.provider == Provider::Claude
                    && [
                        "CLAUDE_CODE_USE_BEDROCK",
                        "CLAUDE_CODE_USE_VERTEX",
                        "CLAUDE_CODE_USE_FOUNDRY",
                    ]
                    .contains(&key.as_str())
                    && matches!(value.to_ascii_lowercase().as_str(), "1" | "true" | "yes")
        })
    }
    #[must_use]
    pub fn refreshed(&self) -> Self {
        let mut context = self.clone();
        context.generation = context.configuration_generation().unwrap_or_default();
        context
    }
    fn configuration_generation(&self) -> Option<Vec<u8>> {
        let mut readable = true;
        let mut encoded = serde_json::to_vec(&self.args).unwrap_or_default();
        // Provider config may interpolate any captured environment variable.
        // Canonicalize order and retain only the resulting in-memory digest.
        let mut environment = self.environment.iter().collect::<Vec<_>>();
        environment.sort_unstable_by(|left, right| left.0.cmp(&right.0));
        for (key, value) in environment {
            encoded.extend_from_slice(key.as_bytes());
            encoded.push(0);
            encoded.extend_from_slice(value.as_bytes());
            encoded.push(0);
        }
        let home = self
            .environment
            .iter()
            .find(|(key, _)| key == "HOME" || key == "USERPROFILE")
            .map(|(_, value)| PathBuf::from(value))
            .unwrap_or_default();
        let store = self
            .environment
            .iter()
            .find(|(key, _)| {
                key == if self.provider == Provider::Codex {
                    "CODEX_HOME"
                } else {
                    "CLAUDE_CONFIG_DIR"
                }
            })
            .map_or_else(
                || {
                    home.join(if self.provider == Provider::Codex {
                        ".codex"
                    } else {
                        ".claude"
                    })
                },
                |(_, value)| PathBuf::from(value),
            );
        let mut paths = match self.provider {
            Provider::Codex => vec![
                store.join("config.toml"),
                self.cwd.join(".codex/config.toml"),
            ],
            Provider::Claude => vec![
                self.cwd.join(".mcp.json"),
                self.cwd.join(".claude/settings.json"),
                self.cwd.join(".claude/settings.local.json"),
                home.join(".claude.json"),
                store.join(".claude.json"),
                store.join("settings.json"),
            ],
        };
        for ancestor in self.cwd.ancestors().skip(1) {
            paths.extend(match self.provider {
                Provider::Codex => vec![ancestor.join(".codex/config.toml")],
                Provider::Claude => vec![
                    ancestor.join(".mcp.json"),
                    ancestor.join(".claude/settings.json"),
                    ancestor.join(".claude/settings.local.json"),
                ],
            });
        }
        for argument in &self.args {
            for prefix in ["--mcp-config=", "--settings="] {
                if let Some(path) = argument.strip_prefix(prefix) {
                    if !path.trim_start().starts_with('{') {
                        paths.push(self.cwd.join(path));
                    }
                }
            }
        }
        for pair in self.args.windows(2) {
            if matches!(pair[0].as_str(), "--mcp-config" | "--settings")
                && !pair[1].trim_start().starts_with('{')
            {
                paths.push(self.cwd.join(&pair[1]));
            }
        }
        for path in paths {
            encoded.extend_from_slice(path.to_string_lossy().as_bytes());
            let opened = fs_err::File::open(path);
            if opened
                .as_ref()
                .is_err_and(|error| error.kind() != std::io::ErrorKind::NotFound)
            {
                readable = false;
            }
            if let Ok(file) = opened {
                let mut contents = Vec::new();
                let read = file.take(1024 * 1024 + 1).read_to_end(&mut contents);
                if read.is_err() || contents.len() > 1024 * 1024 {
                    readable = false;
                    continue;
                }
                encoded.extend_from_slice(
                    ring::digest::digest(&ring::digest::SHA256, &contents).as_ref(),
                );
            }
        }
        readable.then(|| {
            ring::digest::digest(&ring::digest::SHA256, &encoded)
                .as_ref()
                .to_vec()
        })
    }
    fn server_key(&self, name: &str) -> String {
        format!(
            "{}:{}:{}:{:?}:{name}",
            self.identity,
            self.program.display(),
            self.cwd.display(),
            self.generation
        )
    }
    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(&self.program);
        command
            .args(&self.args)
            .args(args)
            .current_dir(&self.cwd)
            .env_clear()
            .envs(self.environment.iter().cloned());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt as _;
            command.process_group(0);
        }
        command
    }
    pub fn account(&self) -> Result<AccountStatus, String> {
        if self.externally_managed() {
            return Ok(AccountStatus::External);
        }
        match self.provider {
            Provider::Codex => {
                let result = query(self.command(&["login", "status"]))?;
                let text = format!("{}\n{}", plain(&result.output), plain(&result.error));
                if result.success && text.contains("API key") {
                    return Ok(AccountStatus::External);
                }
                if result.success && text.contains("Logged in using") {
                    Ok(AccountStatus::Connected {
                        identity: None,
                        method: if text.contains("API key") {
                            "API key managed by Codex"
                        } else {
                            "Browser sign-in"
                        }
                        .into(),
                    })
                } else if text.contains("Not logged in") || text.contains("not logged in") {
                    Ok(AccountStatus::SignedOut)
                } else {
                    Err("Codex did not return a recognized account status. Refresh or check Provider versions.".into())
                }
            }
            Provider::Claude => {
                let result = query(self.command(&["auth", "status", "--json"]))?;
                let value: Value=serde_json::from_slice(&result.output).map_err(|_|"Claude Code did not return structured account status. Refresh or update the provider.".to_owned())?;
                if matches!(value["authMethod"].as_str(), Some("api_key" | "apiKey")) {
                    return Ok(AccountStatus::External);
                }
                match value["loggedIn"].as_bool() {
                    Some(true) => Ok(AccountStatus::Connected {
                        identity: value["email"].as_str().map(safe_label),
                        method: match value["authMethod"].as_str() {
                            Some("api_key" | "apiKey") => "API key managed by Claude",
                            _ => "Provider sign-in",
                        }
                        .into(),
                    }),
                    Some(false) => Ok(AccountStatus::SignedOut),
                    None => Err("Claude Code account status is unavailable".into()),
                }
            }
        }
    }
    /// Explicit inventory request. Claude's CLI enforces its project-server approval
    /// choices. No configuration is edited and no project approval is supplied here.
    pub fn servers(&self) -> Result<Vec<ServerStatus>, String> {
        match self.provider {
            Provider::Codex => {
                let result = query(self.command(&["mcp", "list", "--json"]))?;
                if !result.success {
                    return Err("Could not read Codex MCP servers. Check provider configuration or update the CLI.".into());
                }
                let value: Value = serde_json::from_slice(&result.output)
                    .map_err(|_| "Codex MCP inventory is unavailable".to_owned())?;
                let rows = value
                    .as_array()
                    .ok_or("Codex returned an unsupported MCP inventory")?;
                Ok(rows
                    .iter()
                    .filter_map(|row| {
                        let name = row["name"].as_str()?.to_owned();
                        let enabled = row["enabled"].as_bool().unwrap_or(true);
                        let auth = row["auth_status"].as_str().unwrap_or("unknown");
                        let local = row["transport"]["type"].as_str() == Some("stdio");
                        let external = row["transport"]["bearer_token_env_var"].is_string()
                            || row["transport"]["http_headers"]
                                .as_object()
                                .is_some_and(|h| !h.is_empty());
                        Some(ServerStatus {
                            name,
                            state: if !enabled {
                                ServerState::Disabled
                            } else if auth == "not_logged_in" {
                                ServerState::NeedsAuth
                            } else {
                                ServerState::Unknown
                            },
                            oauth: enabled && !local && !external,
                            authorized: auth == "logged_in",
                            source: if local {
                                "Local stdio"
                            } else {
                                "Provider configuration"
                            }
                            .into(),
                        })
                    })
                    .collect())
            }
            Provider::Claude => {
                let result = query(self.command(&["mcp", "list"]))?;
                if !result.success {
                    return Err("Could not read Claude MCP servers. Check workspace trust, provider configuration or CLI version.".into());
                }
                let mut servers = parse_claude_servers(&plain(&result.output))?;
                if let Ok(grants) = known_oauth().lock() {
                    for server in &mut servers {
                        if grants.contains(&self.server_key(&server.name)) {
                            server.oauth = true;
                            server.authorized = server.state == ServerState::Connected;
                        }
                    }
                }
                Ok(servers)
            }
        }
    }
    /// Explicit connection check; callers must preserve provider trust/approval policy.
    pub fn check_servers(&self) -> Result<Vec<ServerStatus>, String> {
        if self.provider == Provider::Claude {
            return self.servers();
        }
        let mut configured = self.servers()?;
        let rows = self.codex_inventory()?;
        for row in rows {
            let Some(name) = row["name"].as_str() else {
                continue;
            };
            let Some(server) = configured.iter_mut().find(|server| server.name == name) else {
                continue;
            };
            if server.state == ServerState::Disabled {
                continue;
            }
            let auth = row["authStatus"].as_str().unwrap_or("unknown");
            server.authorized = auth == "oAuth";
            server.oauth = matches!(auth, "oAuth" | "notLoggedIn");
            server.state = if auth == "notLoggedIn" {
                ServerState::NeedsAuth
            } else if row["serverInfo"].is_object() && row["toolsError"].is_null() {
                ServerState::Connected
            } else if row["toolsError"].is_string() {
                ServerState::Unavailable
            } else {
                ServerState::Unknown
            };
        }
        Ok(configured)
    }
    fn codex_inventory(&self) -> Result<Vec<Value>, String> {
        let mut child = self
            .command(&["--no-daemon", "app-server", "--stdio"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| "Could not start Codex connection discovery")?;
        let result = (|| {
            let mut input = child
                .stdin
                .take()
                .ok_or("Codex connection input is unavailable")?;
            let output = child
                .stdout
                .take()
                .ok_or("Codex connection output is unavailable")?;
            let (send, recv) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                for line in BufReader::new(output).lines() {
                    let Ok(line) = line else {
                        break;
                    };
                    if line.len() > 1024 * 1024 {
                        break;
                    }
                    if let Ok(value) = serde_json::from_str::<Value>(&line) {
                        if send.send(value).is_err() {
                            break;
                        }
                    }
                }
            });
            let deadline = Instant::now() + QUERY_TIMEOUT;
            let mut request = |id, method: &str, params: Value| -> Result<Value, String> {
                if id == 2 {
                    input
                        .write_all(b"{\"method\":\"initialized\"}\n")
                        .map_err(|_| "Could not initialize Codex discovery")?;
                }
                serde_json::to_writer(
                    &mut input,
                    &serde_json::json!({"id":id,"method":method,"params":params}),
                )
                .map_err(|_| "Could not write Codex connection request")?;
                input
                    .write_all(b"\n")
                    .and_then(|()| input.flush())
                    .map_err(|_| "Could not write Codex connection request")?;
                loop {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    let value = recv
                        .recv_timeout(remaining)
                        .map_err(|_| "Codex connection discovery timed out")?;
                    if value["id"] == id {
                        if !value["error"].is_null() {
                            return Err("This Codex version could not check MCP connections. Check provider policy/configuration or update the CLI.".into());
                        }
                        return Ok(value["result"].clone());
                    }
                }
            };
            request(
                1,
                "initialize",
                serde_json::json!({"clientInfo":{"name":"grove-connections","version":env!("CARGO_PKG_VERSION")},"capabilities":{"experimentalApi":true}}),
            )?;
            let mut rows = Vec::new();
            let mut cursor = Value::Null;
            for page in 0..32 {
                let result = request(
                    2 + page,
                    "mcpServerStatus/list",
                    serde_json::json!({"cursor":cursor,"limit":100}),
                )?;
                let data = result["data"]
                    .as_array()
                    .ok_or("Codex returned an unsupported MCP status inventory")?;
                rows.extend(data.iter().cloned());
                cursor = result["nextCursor"].clone();
                if cursor.is_null() {
                    return Ok(rows);
                }
            }
            Err("Codex returned too many MCP inventory pages".into())
        })();
        kill_query(&mut child);
        result
    }
    pub fn start(&self, action: ConnectionAction) -> Result<AuthJob, String> {
        if self.externally_managed()
            && matches!(action, ConnectionAction::Login | ConnectionAction::Logout)
        {
            return Err("This provider uses externally managed credentials. Grove will not replace or clear them.".into());
        }
        let reservation = Reservation::acquire(self.identity.clone())?;
        let (sender, receiver) = std::sync::mpsc::channel();
        let state = Arc::new(Mutex::new(JobSnapshot {
            phase: JobPhase::Starting,
            output: String::new(),
            authorization_url: None,
            result: None,
        }));
        let cancel = Arc::new(AtomicBool::new(false));
        let job = AuthJob {
            state: state.clone(),
            cancel: cancel.clone(),
            input: sender,
        };
        let context = self.clone();
        std::thread::spawn(move || {
            let reservation = reservation;
            let result = if context.configuration_generation().as_ref() != Some(&context.generation)
            {
                Err("Provider configuration changed or cannot be safely verified. Refresh this connection before changing credentials.".into())
            } else {
                context.run_auth(&action, &state, &cancel, &receiver)
            };
            drop(reservation);
            if let Ok(mut snapshot) = state.lock() {
                snapshot.phase = if cancel.load(Ordering::Acquire) {
                    JobPhase::Canceled
                } else {
                    JobPhase::Finished
                };
                snapshot.output.clear();
                snapshot.authorization_url = None;
                snapshot.result = Some(if cancel.load(Ordering::Acquire) {
                    Err("Sign-in canceled. Refresh to check the provider’s current state.".into())
                } else {
                    result
                });
            }
        });
        Ok(job)
    }
    fn run_auth(
        &self,
        action: &ConnectionAction,
        state: &Arc<Mutex<JobSnapshot>>,
        cancel: &AtomicBool,
        input: &std::sync::mpsc::Receiver<String>,
    ) -> Result<JobResult, String> {
        let mut args: Vec<String> = self.args.clone();
        match (self.provider, action) {
            (Provider::Codex, ConnectionAction::Login) => args.push("login".into()),
            (Provider::Codex, ConnectionAction::Logout) => args.push("logout".into()),
            (Provider::Claude, ConnectionAction::Login) => {
                args.extend(["auth".into(), "login".into()]);
            }
            (Provider::Claude, ConnectionAction::Logout) => {
                args.extend(["auth".into(), "logout".into()]);
            }
            (_, ConnectionAction::ServerLogin(name)) => {
                args.extend(["mcp".into(), "login".into()]);
                if self.provider == Provider::Claude {
                    args.push("--no-browser".into());
                }
                args.extend(["--".into(), name.clone()]);
            }
            (_, ConnectionAction::ServerLogout(name)) => {
                args.extend(["mcp".into(), "logout".into(), "--".into(), name.clone()]);
            }
        }
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 24,
                cols: 100,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|_| "Could not open the provider sign-in interaction".to_owned())?;
        let mut command = CommandBuilder::new(&self.program);
        command.args(args);
        command.cwd(&self.cwd);
        command.env_clear();
        for (k, v) in &self.environment {
            command.env(k, v);
        }
        let mut child = PtyChild(pair.slave.spawn_command(command).map_err(|_| {
            "Could not start the provider sign-in command. Check Provider versions.".to_owned()
        })?);
        drop(pair.slave);
        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|_| "Provider interaction output is unavailable".to_owned())?;
        let mut writer = pair
            .master
            .take_writer()
            .map_err(|_| "Provider interaction input is unavailable".to_owned())?;
        let output_state = Arc::downgrade(state);
        std::thread::spawn(move || {
            let mut buffer = [0; 2048];
            while let Ok(n) = reader.read(&mut buffer) {
                if n == 0 {
                    break;
                }
                let Some(state) = output_state.upgrade() else {
                    break;
                };
                if let Ok(mut s) = state.lock() {
                    if !matches!(s.phase, JobPhase::Starting | JobPhase::Waiting) {
                        continue;
                    }
                    s.output.push_str(&String::from_utf8_lossy(&buffer[..n]));
                    trim_output(&mut s.output);
                    s.authorization_url = authorization_url(&s.output);
                    s.phase = JobPhase::Waiting;
                };
            }
        });
        let start = Instant::now();
        let success = loop {
            if cancel.load(Ordering::Acquire) || start.elapsed() > LOGIN_TIMEOUT {
                let _ = child.0.kill();
                let _ = child.0.wait();
                return Err(if cancel.load(Ordering::Acquire) {
                    "Sign-in canceled"
                } else {
                    "Sign-in timed out. Start a fresh attempt."
                }
                .into());
            }
            while let Ok(value) = input.try_recv() {
                writer
                    .write_all(value.as_bytes())
                    .and_then(|()| writer.write_all(b"\n"))
                    .and_then(|()| writer.flush())
                    .map_err(|_| "The provider no longer accepts input".to_owned())?;
            }
            if let Some(exit) = child
                .0
                .try_wait()
                .map_err(|_| "Could not check the provider process".to_owned())?
            {
                break exit.success();
            }
            std::thread::sleep(Duration::from_millis(40));
        };
        drop(writer);
        drop(pair.master);
        if !success {
            let output = state
                .lock()
                .ok()
                .map(|snapshot| strip_ansi(&snapshot.output))
                .unwrap_or_default();
            return Err(auth_failure(&output));
        }
        if let Ok(mut s) = state.lock() {
            s.phase = JobPhase::Verifying;
            s.output.clear();
            s.authorization_url = None;
        }
        if cancel.load(Ordering::Acquire) {
            return Err("Sign-in canceled".into());
        }
        if matches!(
            action,
            ConnectionAction::ServerLogin(_) | ConnectionAction::ServerLogout(_)
        ) && self.configuration_generation().as_ref() != Some(&self.generation)
        {
            return Err("Provider configuration changed during authentication. Refresh to verify the selected server before continuing.".into());
        }
        match action {
            ConnectionAction::Login => match self.account()? {s@AccountStatus::Connected{..}=>Ok(JobResult::Account(s)), _=>Err("The flow finished, but the provider did not confirm a signed-in account. Refresh to check again.".into())},
            ConnectionAction::Logout => match self.account()? {AccountStatus::SignedOut=>Ok(JobResult::Account(AccountStatus::SignedOut)), _=>Err("The provider did not confirm sign-out. Credentials may be managed externally; refresh status.".into())},
            ConnectionAction::ServerLogin(name)=> {
                let servers=self.check_servers()?;
                let mut server=servers.into_iter().find(|s|&s.name==name).ok_or("The selected server is no longer in this provider configuration")?;
                if server.authorized||server.state==ServerState::Connected {
                    if self.provider==Provider::Claude {server.authorized=true;server.oauth=true;if let Ok(mut grants)=known_oauth().lock(){grants.insert(self.refreshed().server_key(name));}}
                    Ok(JobResult::Server(server))
                } else {Err("Authorization finished, but the provider has not confirmed this server’s sign-in. Refresh its status before retrying.".into())}
            }
            ConnectionAction::ServerLogout(name)=> {
                let server=self.servers()?.into_iter().find(|s|&s.name==name).ok_or("The selected server is no longer configured")?;
                if server.state==ServerState::NeedsAuth {Ok(JobResult::Server(server))}else{Err("The provider did not confirm that the server’s credentials were cleared".into())}
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AccountStatus {
    Connected {
        identity: Option<String>,
        method: String,
    },
    SignedOut,
    External,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServerState {
    Connected,
    NeedsAuth,
    Disabled,
    Approval,
    Unavailable,
    Unknown,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerStatus {
    pub name: String,
    pub state: ServerState,
    pub oauth: bool,
    pub authorized: bool,
    pub source: String,
}
#[derive(Clone)]
pub enum ConnectionAction {
    Login,
    Logout,
    ServerLogin(String),
    ServerLogout(String),
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum JobPhase {
    Starting,
    Waiting,
    Verifying,
    Finished,
    Canceled,
}
#[derive(Clone)]
pub enum JobResult {
    Account(AccountStatus),
    Server(ServerStatus),
}
// Deliberately no Debug or Serialize: output and URLs may contain credentials.
#[derive(Clone)]
pub struct JobSnapshot {
    pub phase: JobPhase,
    pub output: String,
    pub authorization_url: Option<String>,
    pub result: Option<Result<JobResult, String>>,
}
pub struct AuthJob {
    state: Arc<Mutex<JobSnapshot>>,
    cancel: Arc<AtomicBool>,
    input: std::sync::mpsc::Sender<String>,
}
impl AuthJob {
    pub fn snapshot(&self) -> Option<JobSnapshot> {
        self.state.lock().ok().map(|s| s.clone())
    }
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
    }
    pub fn respond(&self, value: String) -> Result<(), String> {
        if value.len() > 8192 || value.contains(['\n', '\r', '\0']) {
            return Err("Enter one provider code or redirect URL".into());
        }
        self.input
            .send(value)
            .map_err(|_| "The provider interaction has ended".into())
    }
}
impl Drop for AuthJob {
    fn drop(&mut self) {
        self.cancel();
    }
}

struct PtyChild(Box<dyn portable_pty::Child + Send + Sync>);
impl std::ops::Deref for PtyChild {
    type Target = dyn portable_pty::Child + Send + Sync;
    fn deref(&self) -> &Self::Target {
        &*self.0
    }
}
impl Drop for PtyChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// New turns and launches wait while Grove owns a credential mutation.
pub fn credentials_busy(agent: Agent) -> bool {
    let prefix = match agent {
        Agent::Codex => "Codex:",
        Agent::Claude => "Claude Code:",
        _ => return false,
    };
    reservations()
        .lock()
        .map_or(true, |keys| keys.iter().any(|key| key.starts_with(prefix)))
}
fn known_oauth() -> &'static Mutex<HashSet<String>> {
    static GRANTS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    GRANTS.get_or_init(Mutex::default)
}
/// Short-lived provider jobs (side chat, titles and suggestions) share admission
/// with credential changes even though they are absent from the session registry.
pub struct ProviderUse(String);
impl ProviderUse {
    pub fn acquire(provider: Provider) -> Result<Self, String> {
        Self::acquire_named(provider.label())
    }
    #[cfg(all(test, unix))]
    pub(crate) fn named_for_test(provider: &str) -> Result<Self, String> {
        Self::acquire_named(provider)
    }
    fn acquire_named(provider: &str) -> Result<Self, String> {
        let mut keys = reservations()
            .lock()
            .map_err(|_| "Provider admission is unavailable")?;
        let prefix = format!("{provider}:");
        if keys.iter().any(|key| key.starts_with(&prefix)) {
            return Err(
                "Wait for the provider’s authentication action to finish. Your draft is preserved."
                    .into(),
            );
        }
        let key = format!("use:{provider}:{}", crate::identity::random_id()?);
        keys.insert(key.clone());
        Ok(Self(key))
    }
}
impl Drop for ProviderUse {
    fn drop(&mut self) {
        if let Ok(mut keys) = reservations().lock() {
            keys.remove(&self.0);
        }
    }
}
#[cfg(all(test, unix))]
pub(crate) fn reserve_named_for_test(provider: &str) -> Result<impl Drop, String> {
    Reservation::acquire(format!("{provider}:fixture-store"))
}
struct Reservation(String);
fn reservations() -> &'static Mutex<HashSet<String>> {
    static LOCKS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    LOCKS.get_or_init(Mutex::default)
}
impl Reservation {
    fn acquire(key: String) -> Result<Self, String> {
        let mut keys = reservations()
            .lock()
            .map_err(|_| "Connection actions are unavailable")?;
        let provider = key.split(':').next().unwrap_or_default();
        if keys
            .iter()
            .any(|key| key.starts_with(&format!("use:{provider}:")))
        {
            return Err("Wait for this provider’s side chat or background request to finish before changing credentials.".into());
        }
        if !keys.insert(key.clone()) {
            return Err(
                "Another authentication action is already running for this provider configuration"
                    .into(),
            );
        }
        Ok(Self(key))
    }
}
impl Drop for Reservation {
    fn drop(&mut self) {
        if let Ok(mut keys) = reservations().lock() {
            keys.remove(&self.0);
        }
    }
}

fn credential_key(provider: Provider, environment: &[(String, String)]) -> String {
    let explicit = environment
        .iter()
        .find(|(key, _)| {
            key == if provider == Provider::Codex {
                "CODEX_HOME"
            } else {
                "CLAUDE_CONFIG_DIR"
            }
        })
        .map(|(_, value)| PathBuf::from(value));
    let home = environment
        .iter()
        .find(|(key, _)| key == "HOME" || key == "USERPROFILE")
        .map(|(_, value)| PathBuf::from(value))
        .unwrap_or_default();
    let store = explicit.unwrap_or_else(|| {
        home.join(if provider == Provider::Codex {
            ".codex"
        } else {
            ".claude"
        })
    });
    let store = fs_err::canonicalize(&store).unwrap_or(store);
    format!("{}:{}", provider.label(), store.display())
}
fn auth_failure(output: &str) -> String {
    let lower = output.to_ascii_lowercase();
    if lower.contains("credential store") {"The provider could not read or update its credential store. Saved credentials may still be active. Refresh status, then explicitly retry clearing sign-in."}
    else if lower.contains("unknown command")||lower.contains("unknown option")||lower.contains("unexpected argument")||lower.contains("unrecognized subcommand") {"This provider version does not support the sign-in action. Update it in Provider versions, then retry."}
    else if lower.contains("denied")||lower.contains("declined")||lower.contains("access_denied") {"Sign-in was not approved. The connection is still unavailable; try again when ready."}
    else if lower.contains("timed out")||lower.contains("timeout") {"The provider’s sign-in timed out. Start a fresh attempt."}
    else {"The provider did not complete authentication. Refresh status before retrying; the connection may have been declined, expired or unavailable."}.into()
}
struct QueryOutput {
    success: bool,
    output: Vec<u8>,
    error: Vec<u8>,
}
fn query(mut command: Command) -> Result<QueryOutput, String> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| "The provider command is unavailable. Check Provider versions.".to_owned())?;
    let stdout = child
        .stdout
        .take()
        .ok_or("Provider output is unavailable")?;
    let stderr = child
        .stderr
        .take()
        .ok_or("Provider error output is unavailable")?;
    let (error_send, error_recv) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stderr.take(1024 * 1024).read_to_end(&mut bytes);
        let _ = error_send.send(bytes);
    });
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stdout.take(1024 * 1024).read_to_end(&mut bytes);
        let _ = sender.send(bytes);
    });
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(exit)) => {
                return Ok(QueryOutput {
                    success: exit.success(),
                    output: receiver
                        .recv_timeout(Duration::from_millis(500))
                        .map_err(|_| "The provider did not finish returning status")?,
                    error: error_recv
                        .recv_timeout(Duration::from_millis(500))
                        .map_err(|_| "The provider did not finish returning status")?,
                })
            }
            Ok(None) if start.elapsed() < QUERY_TIMEOUT => {
                std::thread::sleep(Duration::from_millis(20));
            }
            _ => {
                kill_query(&mut child);
                return Err("Provider status timed out. Refresh to try again.".into());
            }
        }
    }
}
fn kill_query(child: &mut std::process::Child) {
    #[cfg(unix)]
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    let _ = child.kill();
    let _ = child.wait();
}
fn trim_output(s: &mut String) {
    if s.len() > OUTPUT_LIMIT {
        let mut n = s.len() - OUTPUT_LIMIT;
        while !s.is_char_boundary(n) {
            n += 1;
        }
        s.drain(..n);
    }
}
fn plain(bytes: &[u8]) -> String {
    strip_ansi(&String::from_utf8_lossy(bytes))
}
pub fn strip_ansi(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            match chars.next() {
                Some('[') => {
                    for c in chars.by_ref() {
                        if ('@'..='~').contains(&c) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    while let Some(c) = chars.next() {
                        if c == '\u{7}' {
                            break;
                        }
                        if c == '\u{1b}' && chars.next() == Some('\\') {
                            break;
                        }
                    }
                }
                _ => {}
            }
        } else if c == '\n' || c == '\t' || !c.is_control() {
            out.push(c);
        }
    }
    out
}
fn safe_label(value: &str) -> String {
    value
        .chars()
        .filter(|c| !c.is_control())
        .take(200)
        .collect()
}
fn authorization_url(text: &str) -> Option<String> {
    let text = strip_ansi(text);
    text.split_whitespace().find_map(|part| {
        let part = part.trim_matches(['\'', '"', '(', ')', '<', '>']);
        let url = url::Url::parse(part).ok()?;
        if matches!(url.scheme(), "http" | "https")
            && !url.host_str().unwrap_or_default().is_empty()
            && url.username().is_empty()
            && url.password().is_none()
        {
            Some(url.to_string())
        } else {
            None
        }
    })
}
fn parse_claude_servers(text: &str) -> Result<Vec<ServerStatus>, String> {
    let mut rows = Vec::new();
    let mut recognized = false;
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        if line.starts_with("Checking MCP server health") {
            recognized = true;
            continue;
        }
        if line.starts_with("No MCP servers configured.") {
            recognized = true;
            continue;
        }
        let (name, details) = line.split_once(": ").ok_or(
            "Claude returned an unsupported MCP inventory; update the provider before retrying",
        )?;
        let (_, detail) = details
            .rsplit_once(" - ")
            .ok_or("Claude returned an incomplete MCP server status")?;
        if name.is_empty() || name.len() > 200 || name.chars().any(char::is_control) {
            return Err("Claude returned an invalid MCP server identity".into());
        }
        let (state, oauth) = if detail.contains("Needs authentication") || detail == "needs-auth" {
            (ServerState::NeedsAuth, true)
        } else if detail.starts_with("⏸ Pending approval") {
            (ServerState::Approval, false)
        } else if matches!(detail, "✔ Connected" | "✓ Connected") {
            (ServerState::Connected, false)
        } else if detail.contains("Disabled") {
            (ServerState::Disabled, false)
        } else if detail.contains("Failed") || detail.contains("failed") {
            (ServerState::Unavailable, false)
        } else {
            (ServerState::Unknown, false)
        };
        rows.push(ServerStatus {
            name: name.to_owned(),
            state,
            oauth,
            authorized: false,
            source: "Provider configuration".into(),
        });
        recognized = true;
    }
    if recognized {
        Ok(rows)
    } else {
        Err("Claude did not return a recognized MCP inventory".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    fn fake_context(script: &str) -> (tempfile::TempDir, ConnectionContext) {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("provider");
        fs_err::write(&program, format!("#!/bin/sh\n{script}\n")).unwrap();
        fs_err::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        let context = ConnectionContext {
            provider: Provider::Codex,
            cwd: dir.path().to_owned(),
            args: Vec::new(),
            environment: std::env::vars().filter(|(key, _)| key == "PATH").collect(),
            program,
            identity: format!("fixture:{}", dir.path().display()),
            generation: Vec::new(),
        };
        let context = context.refreshed();
        (dir, context)
    }
    fn finished(job: &AuthJob) -> JobSnapshot {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let snapshot = job.snapshot().unwrap();
            if matches!(snapshot.phase, JobPhase::Finished | JobPhase::Canceled) {
                return snapshot;
            }
            assert!(Instant::now() < deadline, "job did not finish");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    #[cfg(unix)]
    #[test]
    fn signed_out_stderr_is_not_a_generic_status_failure() {
        let (_dir, context) = fake_context(
            "if [ \"$1 $2\" = 'login status' ]; then echo 'Not logged in' >&2;exit 1;fi;exit 9",
        );
        assert_eq!(context.account().unwrap(), AccountStatus::SignedOut);
        let (_dir, context) = fake_context("echo 'unknown status' >&2;exit 1");
        assert!(context.account().is_err());
    }
    #[cfg(unix)]
    #[test]
    fn process_exit_never_substitutes_for_account_verification() {
        let (_dir, context) = fake_context(
            "if [ \"$1 $2\" = 'login status' ]; then echo 'Not logged in' >&2;exit 1;fi;exit 0",
        );
        let job = context.start(ConnectionAction::Login).unwrap();
        let state = finished(&job);
        assert!(state.result.unwrap().is_err());
        assert!(state.output.is_empty());
        assert!(state.authorization_url.is_none());
        drop(job);
        let (_dir,context)=fake_context("if [ \"$1 $2\" = 'login status' ]; then if [ -f connected ];then echo 'Logged in using ChatGPT' >&2;else echo 'Not logged in' >&2;exit 1;fi;elif [ \"$1\" = login ];then touch connected;elif [ \"$1\" = logout ];then rm -f connected;fi");
        let job = context.start(ConnectionAction::Login).unwrap();
        assert!(matches!(
            finished(&job).result.unwrap(),
            Ok(JobResult::Account(AccountStatus::Connected { .. }))
        ));
        drop(job);
        let deadline = Instant::now() + Duration::from_secs(1);
        while credentials_busy(Agent::Codex) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        let job = context.start(ConnectionAction::Logout).unwrap();
        assert!(matches!(
            finished(&job).result.unwrap(),
            Ok(JobResult::Account(AccountStatus::SignedOut))
        ));
    }
    #[cfg(unix)]
    #[test]
    fn cancel_reaps_owned_process_and_discards_late_output() {
        let (dir,context)=fake_context("echo 'https://identity.example.test/authorize';sleep 2;echo late;touch must_not_complete");
        let job = context.start(ConnectionAction::Login).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while job.snapshot().unwrap().authorization_url.is_none() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        job.cancel();
        let state = finished(&job);
        assert!(state.phase == JobPhase::Canceled);
        assert!(state.output.is_empty());
        assert!(state.authorization_url.is_none());
        std::thread::sleep(Duration::from_millis(100));
        assert!(job.snapshot().unwrap().phase == JobPhase::Canceled);
        assert!(!dir.path().join("must_not_complete").exists());
    }
    #[test]
    fn malformed_inventory_and_command_words_do_not_invent_connected_status() {
        assert!(parse_claude_servers("unexpected provider response").is_err());
        assert!(parse_claude_servers(
            "No MCP servers configured. Use `claude mcp add` to add a server."
        )
        .unwrap()
        .is_empty());
        let rows = parse_claude_servers("jira: https://Connected.invalid/api - unknown\n").unwrap();
        assert_eq!(rows[0].state, ServerState::Unknown);
        assert!(!rows[0].oauth);
    }
    #[cfg(unix)]
    #[test]
    fn disabled_cloud_flags_do_not_override_browser_authentication() {
        let (_dir, mut context) = fake_context("exit 0");
        context.provider = Provider::Claude;
        context.environment = vec![("CLAUDE_CODE_USE_BEDROCK".into(), "false".into())];
        assert!(!context.externally_managed());
        context.environment[0].1 = "true".into();
        assert!(context.externally_managed());
    }
    #[test]
    fn credential_store_aliases_lock_together_but_profile_proof_does_not() {
        let dir = tempfile::tempdir().unwrap();
        let default = vec![("HOME".into(), dir.path().to_string_lossy().into_owned())];
        for (provider, key, suffix) in [
            (Provider::Codex, "CODEX_HOME", ".codex"),
            (Provider::Claude, "CLAUDE_CONFIG_DIR", ".claude"),
        ] {
            let path = dir.path().join(suffix);
            fs_err::create_dir(&path).unwrap();
            let mut explicit = default.clone();
            explicit.push((key.into(), path.to_string_lossy().into_owned()));
            assert_eq!(
                credential_key(provider, &default),
                credential_key(provider, &explicit)
            );
        }
    }
    #[test]
    fn credential_errors_keep_safe_recovery_without_raw_output() {
        let message =
            auth_failure("Bearer TOP_SECRET could not read credential store at /private/secret");
        assert!(message.contains("credential store"));
        assert!(!message.contains("TOP_SECRET"));
        assert!(!message.contains("/private"));
    }
    #[cfg(unix)]
    #[test]
    fn profile_and_changed_server_configuration_cannot_reuse_grant_proof_or_mutate_stale_selection()
    {
        let (dir, context) = fake_context("exit 0");
        let mut other = context.clone();
        other.args = vec!["--mcp-config".into(), "other.json".into()];
        other = other.refreshed();
        assert_ne!(context.server_key("jira"), other.server_key("jira"));
        fs_err::create_dir_all(dir.path().join(".codex")).unwrap();
        fs_err::write(dir.path().join(".codex/config.toml"), "changed=true").unwrap();
        let job = context.start(ConnectionAction::Logout).unwrap();
        assert!(finished(&job).result.unwrap().is_err());
        assert_ne!(
            context.server_key("jira"),
            context.refreshed().server_key("jira")
        );
    }
    #[test]
    fn background_provider_admission_and_credential_mutation_are_mutually_exclusive() {
        let mutation = Reservation::acquire("admission-fixture:store".into()).unwrap();
        assert!(ProviderUse::acquire_named("admission-fixture").is_err());
        drop(mutation);
        let usage = ProviderUse::acquire_named("admission-fixture").unwrap();
        assert!(Reservation::acquire("admission-fixture:store".into()).is_err());
        drop(usage);
        assert!(Reservation::acquire("admission-fixture:store".into()).is_ok());
    }
    #[cfg(unix)]
    #[test]
    fn unknown_server_state_never_confirms_credential_clearing() {
        let (_dir,context)=fake_context("if [ \"$1 $2\" = 'mcp list' ];then printf '%s\\n' '[{\"name\":\"fixture\",\"enabled\":true,\"auth_status\":\"unknown\",\"transport\":{\"type\":\"streamable_http\"}}]';fi;exit 0");
        let servers = context.servers().unwrap();
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].state, ServerState::Unknown);
        let job = context
            .start(ConnectionAction::ServerLogout("fixture".into()))
            .unwrap();
        assert!(finished(&job).result.unwrap().is_err());
    }
    #[cfg(unix)]
    #[test]
    fn environment_interpolation_is_part_of_credential_context_proof() {
        let (directory, mut first) = fake_context("exit 0");
        first.provider = Provider::Claude;
        fs_err::write(
            directory.path().join(".mcp.json"),
            r#"{"mcpServers":{"jira":{"type":"http","url":"${MCP_URL}"}}}"#,
        )
        .unwrap();
        first
            .environment
            .push(("MCP_URL".into(), "https://first.example.test/mcp".into()));
        first = first.refreshed();
        let mut second = first.clone();
        second
            .environment
            .iter_mut()
            .find(|(key, _)| key == "MCP_URL")
            .unwrap()
            .1 = "https://second.example.test/mcp".into();
        second = second.refreshed();
        assert_ne!(first.server_key("jira"), second.server_key("jira"));
        let mut reordered = first.clone();
        reordered.environment.reverse();
        reordered = reordered.refreshed();
        assert_eq!(first.server_key("jira"), reordered.server_key("jira"));
    }
    #[test]
    fn inventory_keeps_only_names_and_status_not_credentials() {
        let rows=parse_claude_servers("Checking MCP server health...\njira: https://secret.invalid?token=DO_NOT_KEEP (HTTP) - ✓ Connected\nprivate: secret_command --password hidden - ! Needs authentication\nunapproved: /evil/command - ⏸ Pending approval\n");
        let rows = rows.unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].name, "jira");
        assert_eq!(rows[0].state, ServerState::Connected);
        assert_eq!(rows[1].state, ServerState::NeedsAuth);
        assert_eq!(rows[2].state, ServerState::Approval);
        assert!(!format!("{rows:?}").contains("DO_NOT_KEEP"));
        assert!(!format!("{rows:?}").contains("evil"));
    }
    #[test]
    fn shared_store_rejects_duplicate_mutations_until_release() {
        let reservation = Reservation::acquire("test-store".into()).unwrap();
        assert!(Reservation::acquire("test-store".into()).is_err());
        drop(reservation);
        assert!(Reservation::acquire("test-store".into()).is_ok());
    }
    #[test]
    fn browser_urls_reject_active_schemes_and_strip_terminal_sequences() {
        assert_eq!(
            authorization_url("\x1b[31mhttps://identity.example.test/auth?state=abc\x1b[0m"),
            Some("https://identity.example.test/auth?state=abc".into())
        );
        assert!(authorization_url("file:///tmp/secret javascript:alert(1)").is_none());
        assert!(authorization_url("https://user:secret@example.test").is_none());
        assert_eq!(strip_ansi("\x1b[31mhello\x1b[0m\r\n"), "hello\n");
    }
    #[test]
    fn output_is_bounded_on_unicode_boundaries() {
        let mut text = "界".repeat(50000);
        trim_output(&mut text);
        assert!(text.len() <= OUTPUT_LIMIT);
    }
}
