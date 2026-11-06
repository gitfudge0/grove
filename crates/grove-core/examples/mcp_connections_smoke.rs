//! Live MCP failure visibility without model requests or Jira credentials.
//! Run with a disposable GROVE_CONFIG_DIR and cwd; uses the production transports.
use grove_core::provider::{self, Provider, SavedSession, Status};
use serde_json::json;
use std::time::{Duration, Instant};

struct Cleanup(provider::Handle);
impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = self
            .0
            .stop_completion(false)
            .recv_timeout(Duration::from_secs(5));
    }
}
fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).is_some_and(|arg| arg == "--broken-mcp") {
        return Err("Intentional MCP startup failure".into());
    }
    if std::env::var_os("GROVE_CONFIG_DIR").is_none() {
        return Err("Set GROVE_CONFIG_DIR to a disposable directory".into());
    }
    let provider = match args.get(1).map(String::as_str) {
        Some("codex") => Provider::Codex,
        Some("claude") => Provider::Claude,
        _ => return Err("Usage: mcp_connections_smoke codex|claude DISPOSABLE_CWD".into()),
    };
    let cwd = args.get(2).ok_or("Missing disposable cwd")?;
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let command = serde_json::to_string(&executable).map_err(|error| error.to_string())?;
    let config_args = match provider {
        Provider::Codex => vec!["-c".to_owned(), format!("mcp_servers.grove_failure_probe={{command={command},args=[\"--broken-mcp\"],startup_timeout_sec=3}}")],
        Provider::Claude => vec!["--strict-mcp-config".to_owned(), "--mcp-config".to_owned(), json!({"mcpServers":{"grove_failure_probe":{"command":executable,"args":["--broken-mcp"]}}}).to_string()],
    };
    grove_core::env_path::ensure_login_path();
    let saved: SavedSession = serde_json::from_value(json!({
        "provider":provider,"key":grove_core::identity::random_id()?,
        "project":"MCP smoke","cwd":cwd,"label":"MCP smoke",
        "roots":[],"thread_id":null,"items":[],"config_args":config_args,"native_config_version":1
    }))
    .map_err(|error| error.to_string())?;
    let connection = grove_core::codex::connect(saved, Vec::new());
    let _cleanup = Cleanup(connection.handle.clone());
    let deadline = Instant::now() + Duration::from_secs(40);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err("No visible MCP failure before deadline".into());
        }
        let state = connection
            .updates
            .recv_timeout(remaining)
            .map_err(|error| error.to_string())?;
        if matches!(state.status, Status::Failed | Status::Disconnected) {
            return Err(format!(
                "Conversation failed instead of reporting MCP health: {:?}",
                state.error
            ));
        }
        let warning = state.saved.items.iter().find(|item| {
            item["type"] == "providerNotice"
                && item["title"] == "MCP server grove_failure_probe is unavailable"
        });
        if let Some(warning) = warning.filter(|_| state.status == Status::Idle) {
            if state.error.is_some() {
                return Err("MCP health became a fatal session error".into());
            }
            println!(
                "PASS {provider:?}: visible startup failure while conversation stays Idle: {}",
                warning["text"]
            );
            return Ok(());
        }
    }
}
