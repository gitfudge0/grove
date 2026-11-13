//! Read-only status or isolated localhost OAuth fixture through the production service.
//! Login modes refuse non-loopback authorization URLs and never print links/codes/output.
use grove_core::{
    connections::{AccountStatus, ConnectionAction, ConnectionContext, JobPhase, JobResult},
    provider::Provider,
};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let provider=match args.next().as_deref(){Some("codex")=>Provider::Codex,Some("claude")=>Provider::Claude,_=>return Err("usage: provider_connections_smoke codex|claude CWD [status|servers|fixture-login|fixture-cancel|fixture-clear] [SERVER]".into())};
    let cwd = PathBuf::from(args.next().ok_or("CWD required")?);
    let mode = args.next().unwrap_or_else(|| "status".into());
    let context = ConnectionContext::new(provider, cwd, Vec::new())?;
    match mode.as_str() {
        "status" => {
            let account = context.account()?;
            let state = match account {
                AccountStatus::SignedOut => "signed-out",
                AccountStatus::External => "external",
                AccountStatus::Connected { .. } => "connected",
            };
            println!("provider={} account={state}", provider.label());
        }
        "servers" => {
            for server in context.check_servers()? {
                println!(
                    "server={} state={:?} authorized={} oauth={}",
                    server.name, server.state, server.authorized, server.oauth
                );
            }
        }
        "fixture-login" | "fixture-cancel" | "fixture-clear" => {
            if std::env::var_os("GROVE_CONNECTIONS_ISOLATED_FIXTURE").is_none() {
                return Err("fixture mutation modes require GROVE_CONNECTIONS_ISOLATED_FIXTURE and isolated provider homes".into());
            }
            let name = args.next().ok_or("fixture server required")?;
            let action = if mode == "fixture-clear" {
                ConnectionAction::ServerLogout(name)
            } else {
                ConnectionAction::ServerLogin(name)
            };
            let job = context.start(action)?;
            let deadline = Instant::now() + Duration::from_mins(1);
            let mut opened = false;
            loop {
                if Instant::now() > deadline {
                    job.cancel();
                    return Err("fixture interaction timed out".into());
                }
                if let Some(snapshot) = job.snapshot() {
                    if let Some(url) = snapshot.authorization_url.filter(|_| !opened) {
                        let parsed = url::Url::parse(&url)?;
                        if !matches!(parsed.host_str(), Some("127.0.0.1" | "localhost" | "[::1]")) {
                            job.cancel();
                            return Err("fixture refused a non-loopback authorization URL".into());
                        }
                        if mode == "fixture-cancel" {
                            job.cancel();
                        } else {
                            let _response = ureq::get(&url).call()?;
                        }
                        opened = true;
                    }
                    if matches!(snapshot.phase, JobPhase::Canceled) {
                        println!("provider={} result=canceled", provider.label());
                        break;
                    }
                    if snapshot.phase == JobPhase::Finished {
                        match snapshot.result.ok_or("missing result")?? {
                            JobResult::Server(server) => println!(
                                "provider={} state={:?} authorized={}",
                                provider.label(),
                                server.state,
                                server.authorized
                            ),
                            JobResult::Account(_) => return Err("expected server result".into()),
                        }
                        break;
                    }
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        _ => return Err("unknown mode".into()),
    }
    Ok(())
}
