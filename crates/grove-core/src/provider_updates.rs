//! Blocking provider discovery and explicit, installer-owned updates. Call off the UI thread.
use crate::agent::Agent;
use serde::{Deserialize, Serialize};
use std::io::{Read, Seek, SeekFrom};
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

pub const PROVIDERS: [Agent; 3] = [Agent::Codex, Agent::Claude, Agent::OpenCode];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Installer {
    Npm {
        executable: PathBuf,
        prefix: PathBuf,
        package: String,
    },
    /// The official versioned native installation; its updater owns channel selection.
    ClaudeNative,
    External {
        guidance: String,
    },
}
impl Installer {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Npm { .. } => "npm",
            Self::ClaudeNative => "Claude native",
            Self::External { .. } => "External installation",
        }
    }
    pub fn guidance(&self) -> &str {
        match self {
            Self::Npm { .. } => "Managed by npm. Grove updates only this installation.",
            Self::ClaudeNative => "Managed by Claude Code’s native updater.",
            Self::External { guidance } => guidance,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProviderStatus {
    pub agent: Agent,
    pub installed: Option<String>,
    pub latest: Option<String>,
    pub executable: Option<PathBuf>,
    pub installer: Installer,
    pub error: Option<String>,
}
impl ProviderStatus {
    pub fn update_available(&self) -> bool {
        self.installed
            .as_deref()
            .and_then(version)
            .zip(self.latest.as_deref().and_then(version))
            .is_some_and(|(a, b)| b > a)
    }
    pub fn can_update(&self) -> bool {
        match self.installer {
            Installer::ClaudeNative => self.installed.as_deref().and_then(version).is_some(),
            Installer::Npm { .. } => self.update_available(),
            Installer::External { .. } => false,
        }
    }
}
fn package(agent: Agent) -> Option<&'static str> {
    match agent {
        Agent::Codex => Some("@openai/codex"),
        Agent::Claude => Some("@anthropic-ai/claude-code"),
        Agent::OpenCode => Some("opencode-ai"),
        Agent::Terminal => None,
    }
}
fn version(text: &str) -> Option<semver::Version> {
    text.split_whitespace()
        .find_map(|word| semver::Version::parse(word.trim_start_matches('v')).ok())
}
fn external(agent: Agent) -> Installer {
    Installer::External {
        guidance: format!(
            "Update {} using the installer or package manager that installed it, then check again.",
            agent.label()
        ),
    }
}
fn resolve_npm() -> Option<PathBuf> {
    let inherited = std::env::var_os("PATH").unwrap_or_default();
    let environment = crate::env_path::session_environment();
    let path = environment
        .as_ref()
        .and_then(|env| env.iter().find(|(key, _)| key == "PATH"))
        .map_or(inherited, |(_, value)| value.into());
    std::env::split_paths(&path)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join("npm"))
        .find(|candidate| {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs_err::metadata(candidate).is_ok_and(|metadata| {
                    metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
                })
            }
            #[cfg(not(unix))]
            {
                candidate.is_file()
            }
        })
}
fn installer(agent: Agent, resolved: &Path) -> Installer {
    installer_with_npm(agent, resolved, resolve_npm().as_deref())
}
fn installer_with_npm(agent: Agent, resolved: &Path, fallback_npm: Option<&Path>) -> Installer {
    let Some(pkg) = package(agent) else {
        return external(agent);
    };
    // Only known package entry points establish ownership; a nearby npm binary is insufficient.
    let suffix = match agent {
        Agent::Codex => "lib/node_modules/@openai/codex/bin/codex.js",
        Agent::Claude => "lib/node_modules/@anthropic-ai/claude-code/cli.js",
        Agent::OpenCode => "lib/node_modules/opencode-ai/bin/opencode",
        Agent::Terminal => return external(agent),
    };
    if let Some(prefix) = resolved
        .to_str()
        .and_then(|s| s.strip_suffix(suffix))
        .filter(|prefix| prefix.ends_with(std::path::MAIN_SEPARATOR))
        .map(PathBuf::from)
    {
        let local_npm = prefix.join("bin/npm");
        let npm = if local_npm.is_file() {
            Some(local_npm)
        } else {
            fallback_npm
                .filter(|path| path.is_absolute() && path.is_file())
                .map(Path::to_path_buf)
        };
        if let Some(npm) = npm.filter(|_| prefix.is_absolute()) {
            return Installer::Npm {
                executable: npm,
                prefix,
                package: pkg.into(),
            };
        }
    }
    if agent == Agent::Claude
        && dirs::home_dir().is_some_and(|home| {
            resolved.parent() == Some(home.join(".local/share/claude/versions").as_path())
                && resolved
                    .file_name()
                    .and_then(|s| s.to_str())
                    .and_then(|name| version(name.strip_suffix(".exe").unwrap_or(name)))
                    .is_some()
        })
    {
        return Installer::ClaudeNative;
    }
    external(agent)
}

// The native updater only owns its standard launcher. A custom symlink or
// direct version-file invocation would keep launching the old version afterward.
fn installer_for_invocation(agent: Agent, launcher: &Path, resolved: &Path) -> Installer {
    let detected = installer(agent, resolved);
    if matches!(detected, Installer::ClaudeNative)
        && !dirs::home_dir().is_some_and(|home| {
            launcher
                == home.join(".local/bin").join(if cfg!(windows) {
                    "claude.exe"
                } else {
                    "claude"
                })
        })
    {
        return external(agent);
    }
    detected
}

/// Return the actual resolved provider, including Windows npm command wrappers.
/// Only accept the fixed wrapper shape emitted by Agent::invocation().
fn invocation_path(program: &str, args: &[String]) -> Option<PathBuf> {
    let path = if args.is_empty() {
        PathBuf::from(program)
    } else if program.eq_ignore_ascii_case("cmd.exe") && args.len() == 2 && args[0] == "/C" {
        let wrapper = &args[1];
        // cmd parses these characters even when invoked without an explicit shell.
        if wrapper.chars().any(|c| "&|<>^%!\"\r\n".contains(c)) {
            return None;
        }
        let path = PathBuf::from(wrapper);
        if !path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("cmd") || e.eq_ignore_ascii_case("bat"))
        {
            return None;
        }
        path
    } else {
        return None;
    };
    path.is_absolute().then_some(path)
}
fn run(mut command: Command, timeout: Duration) -> Result<String, String> {
    let stdout = tempfile::tempfile().map_err(|e| e.to_string())?;
    let stderr = tempfile::tempfile().map_err(|e| e.to_string())?;
    let mut output = stdout.try_clone().map_err(|e| e.to_string())?;
    let mut errors = stderr.try_clone().map_err(|e| e.to_string())?;
    command.stdin(Stdio::null()).stdout(stdout).stderr(stderr);
    if let Some(env) = crate::env_path::session_environment() {
        command.envs(env.iter().map(|(k, v)| (k, v)));
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command.spawn().map_err(|e| e.to_string())?;
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        let oversized = output.metadata().is_ok_and(|m| m.len() > 1_048_576)
            || errors.metadata().is_ok_and(|m| m.len() > 1_048_576);
        if oversized || started.elapsed() >= timeout {
            #[cfg(unix)]
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGKILL);
            }
            let _ = child.kill();
            let _ = child.wait();
            return Err(if oversized { "The provider command produced too much output. Check the installation before retrying." } else { "The provider command timed out. Check the installation before retrying." }.into());
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    output.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    errors.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    let mut text = String::new();
    output
        .take(65536)
        .read_to_string(&mut text)
        .map_err(|e| e.to_string())?;
    if !status.success() {
        let mut detail = String::new();
        let _ = errors.take(4096).read_to_string(&mut detail);
        return Err(format!(
            "Provider command failed ({status}). {}",
            detail.trim()
        ));
    }
    Ok(text)
}
pub fn discover(agent: Agent) -> ProviderStatus {
    let mut status = ProviderStatus {
        agent,
        installed: None,
        latest: None,
        executable: None,
        installer: external(agent),
        error: None,
    };
    if package(agent).is_none() {
        return status;
    }
    let (program, args) = agent.invocation();
    let Some(path) = invocation_path(&program, &args).filter(|path| path.is_file()) else {
        return status;
    };
    status.executable = Some(path.clone());
    if args.is_empty() {
        if let Ok(canonical) = path.canonicalize() {
            status.installer = installer_for_invocation(agent, &path, &canonical);
        }
    }
    let mut command = Command::new(program);
    command.args(args).arg("--version");
    match run(command, Duration::from_secs(15)) {
        Ok(output) => {
            status.installed = version(&output).map(|v| v.to_string());
            if status.installed.is_none() {
                status.error = Some("Could not read the installed version.".into());
            }
        }
        Err(error) => status.error = Some(error),
    }
    status
}
pub fn latest(agent: Agent) -> Result<String, String> {
    let pkg = package(agent).ok_or("Terminal is not an updatable provider")?;
    let client: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(20)))
        .build()
        .into();
    let body: serde_json::Value = client
        .get(format!("https://registry.npmjs.org/{pkg}/latest"))
        .call()
        .map_err(|e| e.to_string())?
        .body_mut()
        .read_json()
        .map_err(|e| e.to_string())?;
    body.get("version")
        .and_then(|v| v.as_str())
        .and_then(version)
        .map(|v| v.to_string())
        .ok_or_else(|| "The provider registry returned an invalid version.".into())
}
pub fn check(agent: Agent) -> ProviderStatus {
    let mut status = discover(agent);
    if matches!(status.installer, Installer::ClaudeNative) {
        return status;
    }
    match latest(agent) {
        Ok(v) => status.latest = Some(v),
        Err(e) => status.error = Some(format!("Could not check for updates: {e}")),
    }
    status
}
pub fn check_all() -> Vec<ProviderStatus> {
    PROVIDERS.into_iter().map(check).collect()
}

pub fn update(request: &ProviderStatus) -> Result<ProviderStatus, String> {
    update_with(request, discover, latest, |command| {
        run(command, Duration::from_mins(5))
    })
}

fn update_with(
    request: &ProviderStatus,
    mut discover_provider: impl FnMut(Agent) -> ProviderStatus,
    mut latest_version: impl FnMut(Agent) -> Result<String, String>,
    mut execute: impl FnMut(Command) -> Result<String, String>,
) -> Result<ProviderStatus, String> {
    // Never trust persisted installer details or a stale PATH resolution to authorize mutation.
    let fresh = discover_provider(request.agent);
    if fresh.executable != request.executable || fresh.installer != request.installer {
        return Err("The provider installation changed. Check for updates again.".into());
    }
    if matches!(fresh.installer, Installer::ClaudeNative) {
        // The native updater selects the configured channel, not npm's public latest.
        // Invoke the stable launcher, never its versioned symlink target.
        let before = fresh
            .installed
            .as_deref()
            .and_then(version)
            .ok_or("Could not read the installed version. Check for updates again.")?;
        let launcher = fresh
            .executable
            .as_ref()
            .ok_or("The provider executable is unavailable.")?;
        let mut command = Command::new(launcher);
        command.arg("update");
        execute(command)?;
        let mut result = discover_provider(request.agent);
        if result.executable != fresh.executable
            || result.installer != fresh.installer
            || result
                .installed
                .as_deref()
                .and_then(version)
                .is_none_or(|v| v < before)
        {
            return Err(
                "The updater finished, but Grove could not verify the installation. Check again."
                    .into(),
            );
        }
        result.latest = None;
        return Ok(result);
    }
    let target = request
        .latest
        .as_deref()
        .and_then(version)
        .ok_or("Check for updates before updating.")?;
    if latest_version(request.agent)? != target.to_string() {
        return Err("The latest version changed. Check for updates again.".into());
    }
    if fresh
        .installed
        .as_deref()
        .and_then(version)
        .is_none_or(|v| v >= target)
    {
        return Err("This installation does not need the selected update.".into());
    }
    let mut command = match &fresh.installer {
        Installer::Npm {
            executable,
            prefix,
            package,
        } => {
            let mut c = Command::new(executable);
            c.args(["install", "--global", "--prefix"])
                .arg(prefix)
                .arg(format!("{package}@{target}"))
                .args([
                    "--no-audit",
                    "--no-fund",
                    "--registry=https://registry.npmjs.org",
                ]);
            if let Some(scope) = package.strip_prefix('@').and_then(|p| p.split('/').next()) {
                c.arg(format!("--@{scope}:registry=https://registry.npmjs.org"));
            }
            c
        }
        Installer::ClaudeNative => unreachable!("native updater handled above"),
        Installer::External { guidance } => return Err(guidance.clone()),
    };
    command.env("CI", "1");
    execute(command)?;
    let mut result = discover_provider(request.agent);
    if result.executable != fresh.executable
        || result.installer != fresh.installer
        || result.installed.as_deref().and_then(version) != Some(target.clone())
    {
        return Err(format!(
            "The update command finished, but Grove could not verify version {target}. Check the installation and retry."
        ));
    }
    result.latest = Some(target.to_string());
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invocation_resolves_direct_and_windows_wrapper_paths() {
        let dir = tempfile::tempdir().unwrap();
        let direct = dir.path().join("codex");
        let wrapper = dir.path().join("codex.cmd");
        assert_eq!(invocation_path(direct.to_str().unwrap(), &[]), Some(direct));
        assert_eq!(
            invocation_path("cmd.exe", &["/C".into(), wrapper.display().to_string()]),
            Some(wrapper)
        );
        assert!(invocation_path("codex", &[]).is_none());
        assert!(invocation_path("cmd.exe", &["/C".into(), "/tmp/evil&run.cmd".into()]).is_none());
        assert!(invocation_path("cmd.exe", &["/C".into(), "/tmp/codex.exe".into()]).is_none());
        assert!(invocation_path("sh", &["-c".into(), "/tmp/codex.cmd".into()]).is_none());
    }
    fn managed_status(installed: &str) -> ProviderStatus {
        ProviderStatus {
            agent: Agent::Codex,
            installed: Some(installed.into()),
            latest: Some("2.0.0".into()),
            executable: Some(PathBuf::from("/managed/bin/codex")),
            installer: Installer::Npm {
                executable: PathBuf::from("/managed/bin/npm"),
                prefix: PathBuf::from("/managed"),
                package: "@openai/codex".into(),
            },
            error: None,
        }
    }
    fn native_status(installed: &str) -> ProviderStatus {
        ProviderStatus {
            agent: Agent::Claude,
            installer: Installer::ClaudeNative,
            executable: Some("/home/user/.local/bin/claude".into()),
            installed: Some(installed.into()),
            latest: None,
            error: None,
        }
    }
    #[test]
    fn native_update_uses_owned_launcher_and_accepts_configured_channel() {
        for after in ["1.0.0", "1.1.0"] {
            let request = native_status("1.0.0");
            assert!(request.can_update());
            let mut discoveries = [request.clone(), native_status(after)].into_iter();
            let result = update_with(
                &request,
                |_| discoveries.next().unwrap(),
                |_| panic!("native channel must not use npm latest"),
                |command| {
                    assert_eq!(command.get_program(), "/home/user/.local/bin/claude");
                    assert_eq!(command.get_args().collect::<Vec<_>>(), ["update"]);
                    Ok(String::new())
                },
            )
            .unwrap();
            assert_eq!(result.installed.as_deref(), Some(after));
            assert!(result.latest.is_none());
        }
    }
    #[test]
    fn native_update_rejects_failed_commands_changed_identity_and_unverified_versions() {
        let request = native_status("1.0.0");
        assert!(update_with(
            &request,
            |_| request.clone(),
            |_| panic!(),
            |_| Err("permission denied".into())
        )
        .unwrap_err()
        .contains("permission denied"));
        for installed in [Some("0.9.0"), None] {
            let mut after = native_status("1.1.0");
            after.installed = installed.map(str::to_string);
            let mut discoveries = [request.clone(), after].into_iter();
            assert!(update_with(
                &request,
                |_| discoveries.next().unwrap(),
                |_| panic!(),
                |_| Ok(String::new())
            )
            .unwrap_err()
            .contains("could not verify"));
        }
        let mut changed = native_status("1.1.0");
        changed.installer = external(Agent::Claude);
        let mut discoveries = [request.clone(), changed].into_iter();
        assert!(update_with(
            &request,
            |_| discoveries.next().unwrap(),
            |_| panic!(),
            |_| Ok(String::new())
        )
        .is_err());
    }
    #[test]
    fn only_versioned_claude_native_paths_authorize_native_updater() {
        let home = dirs::home_dir().unwrap();
        let directory = home.join(".local/share/claude/versions");
        assert_eq!(
            installer_with_npm(Agent::Claude, &directory.join("2.1.312"), None),
            Installer::ClaudeNative
        );
        assert!(matches!(
            installer_with_npm(Agent::Claude, &directory.join("unrelated"), None),
            Installer::External { .. }
        ));
        assert!(matches!(
            installer_with_npm(Agent::Codex, &directory.join("2.1.312"), None),
            Installer::External { .. }
        ));
    }
    #[test]
    fn native_updater_requires_the_installer_owned_launcher() {
        let home = dirs::home_dir().unwrap();
        let resolved = home.join(".local/share/claude/versions/2.1.312");
        let launcher = home.join(".local/bin").join(if cfg!(windows) {
            "claude.exe"
        } else {
            "claude"
        });
        assert_eq!(
            installer_for_invocation(Agent::Claude, &launcher, &resolved),
            Installer::ClaudeNative
        );
        for custom in [home.join("custom/claude"), resolved.clone()] {
            assert!(matches!(
                installer_for_invocation(Agent::Claude, &custom, &resolved),
                Installer::External { .. }
            ));
        }
    }
    #[test]
    fn update_targets_exact_owned_prefix_package_and_verifies_version() {
        let request = managed_status("1.0.0");
        let mut discoveries = [managed_status("1.0.0"), managed_status("2.0.0")].into_iter();
        let result = update_with(
            &request,
            |_| discoveries.next().unwrap(),
            |_| Ok("2.0.0".into()),
            |command| {
                assert_eq!(command.get_program(), "/managed/bin/npm");
                let args: Vec<_> = command
                    .get_args()
                    .map(|a| a.to_string_lossy().into_owned())
                    .collect();
                assert_eq!(
                    &args[..5],
                    &[
                        "install",
                        "--global",
                        "--prefix",
                        "/managed",
                        "@openai/codex@2.0.0"
                    ]
                );
                assert!(args
                    .iter()
                    .any(|s| s == "--registry=https://registry.npmjs.org"));
                Ok(String::new())
            },
        )
        .unwrap();
        assert_eq!(result.installed.as_deref(), Some("2.0.0"));
    }
    #[test]
    fn update_cannot_report_success_when_version_verification_fails() {
        let request = managed_status("1.0.0");
        let result = update_with(
            &request,
            |_| managed_status("1.0.0"),
            |_| Ok("2.0.0".into()),
            |_| Ok(String::new()),
        );
        assert!(result.unwrap_err().contains("could not verify"));
    }
    #[test]
    fn update_rejects_changed_owner_even_when_version_matches() {
        let request = managed_status("1.0.0");
        let mut after = managed_status("2.0.0");
        after.installer = external(Agent::Codex);
        let mut discoveries = [request.clone(), after].into_iter();
        let result = update_with(
            &request,
            |_| discoveries.next().unwrap(),
            |_| Ok("2.0.0".into()),
            |_| Ok(String::new()),
        );
        assert!(result.unwrap_err().contains("could not verify"));
    }
    #[test]
    fn changed_identity_or_target_never_reaches_mutation() {
        let request = managed_status("1.0.0");
        let mut changed = request.clone();
        changed.executable = Some(PathBuf::from("/other/codex"));
        assert!(update_with(
            &request,
            |_| changed.clone(),
            |_| panic!("identity change must stop before network"),
            |_| panic!("must not mutate")
        )
        .is_err());
        assert!(update_with(
            &request,
            |_| request.clone(),
            |_| Ok("3.0.0".into()),
            |_| panic!("changed target must not mutate")
        )
        .is_err());
    }
    #[test]
    fn versions_are_strict_and_ordered() {
        assert_eq!(version("codex-cli 0.42.0\n").unwrap().to_string(), "0.42.0");
        assert!(version("unknown 123").is_none());
        let mut s = ProviderStatus {
            agent: Agent::Codex,
            installed: Some("1.9.0".into()),
            latest: Some("1.10.0".into()),
            executable: None,
            installer: external(Agent::Codex),
            error: None,
        };
        assert!(s.update_available());
        assert!(!s.can_update());
        s.latest = Some("1.8.0".into());
        assert!(!s.update_available());
    }
    #[test]
    fn unknown_paths_never_authorize_mutation() {
        for agent in PROVIDERS {
            assert!(matches!(
                installer(agent, Path::new("/usr/local/bin/codex")),
                Installer::External { .. }
            ));
        }
        assert!(matches!(
            installer(
                Agent::Codex,
                Path::new("/tmp/node_modules/@openai/codex/bin/codex.js")
            ),
            Installer::External { .. }
        ));
    }
    #[test]
    fn npm_requires_exact_package_entry_and_prefix() {
        let dir = tempfile::tempdir().unwrap();
        fs_err::create_dir_all(dir.path().join("bin")).unwrap();
        fs_err::write(dir.path().join("bin/npm"), "").unwrap();
        assert!(matches!(
            installer(
                Agent::Codex,
                &dir.path()
                    .join("lib/node_modules/@openai/codex/bin/codex.js")
            ),
            Installer::Npm { .. }
        ));
        assert!(matches!(
            installer(
                Agent::Claude,
                &dir.path()
                    .join("lib/node_modules/@openai/codex/bin/codex.js")
            ),
            Installer::External { .. }
        ));
    }
    #[test]
    fn custom_global_prefix_can_use_a_different_resolved_npm() {
        let dir = tempfile::tempdir().unwrap();
        let npm = dir.path().join("system/bin/npm");
        fs_err::create_dir_all(npm.parent().unwrap()).unwrap();
        fs_err::write(&npm, "").unwrap();
        let prefix = dir.path().join("custom");
        let provider = prefix.join("lib/node_modules/@openai/codex/bin/codex.js");
        assert_eq!(
            installer_with_npm(Agent::Codex, &provider, Some(&npm)),
            Installer::Npm {
                executable: npm,
                prefix,
                package: "@openai/codex".into()
            }
        );
        assert!(matches!(
            installer_with_npm(Agent::Codex, &provider, None),
            Installer::External { .. }
        ));
    }
    #[test]
    fn npm_owner_suffix_requires_a_component_boundary() {
        let dir = tempfile::tempdir().unwrap();
        let apparent_prefix = dir.path().join("evil");
        fs_err::create_dir_all(apparent_prefix.join("bin")).unwrap();
        fs_err::write(apparent_prefix.join("bin/npm"), "").unwrap();
        let unrelated = dir
            .path()
            .join("evillib/node_modules/@openai/codex/bin/codex.js");
        assert!(matches!(
            installer(Agent::Codex, &unrelated),
            Installer::External { .. }
        ));
    }
    #[test]
    fn stale_cached_identity_is_rejected_before_network_or_mutation() {
        let request = ProviderStatus {
            agent: Agent::Terminal,
            installed: Some("1.0.0".into()),
            latest: Some("2.0.0".into()),
            executable: Some(PathBuf::from("/untrusted/cached/provider")),
            installer: external(Agent::Terminal),
            error: None,
        };
        assert!(update(&request)
            .unwrap_err()
            .contains("installation changed"));
    }
    #[test]
    fn excessive_command_output_is_stopped() {
        #[cfg(unix)]
        {
            let mut command = Command::new("/bin/sh");
            command.args(["-c", "while :; do printf '0123456789012345678901234567890123456789012345678901234567890123456789'; done"]);
            assert!(run(command, Duration::from_secs(5)).is_err());
        }
    }
    #[test]
    fn command_timeout_is_bounded() {
        #[cfg(unix)]
        {
            let mut command = Command::new("/bin/sleep");
            command.arg("5");
            assert!(run(command, Duration::from_millis(20))
                .unwrap_err()
                .contains("timed out"));
        }
    }
}
