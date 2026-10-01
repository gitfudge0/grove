//! Exported shell environment recovery for GUI launches. Resolve once before
//! spawning agents to avoid repeating interactive shell initialization.

use std::process::Command;
use std::time::Duration;

use std::ffi::OsString;
use std::sync::{Arc, Mutex, OnceLock, RwLock};

type Environment = Arc<[(String, String)]>;

#[derive(Default)]
struct EnvironmentCache(RwLock<Option<Environment>>);

impl EnvironmentCache {
    fn snapshot(&self) -> Option<Environment> {
        self.0
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn publish(&self, captured: Option<Vec<(String, String)>>) -> Result<(), String> {
        let env = captured.ok_or_else(|| "Could not refresh the shell environment.".to_string())?;
        *self
            .0
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(env.into());
        Ok(())
    }
}

static SESSION_ENV: OnceLock<EnvironmentCache> = OnceLock::new();
static BASELINE_ENV: OnceLock<Vec<(OsString, OsString)>> = OnceLock::new();
static INITIALIZED: OnceLock<()> = OnceLock::new();
static REFRESH_LOCK: Mutex<()> = Mutex::new(());

fn inherited_environment() -> &'static [(OsString, OsString)] {
    BASELINE_ENV.get_or_init(|| std::env::vars_os().collect())
}

/// Capture original inherited exports before startup resolution changes them.
/// Rich terminal launches already inherit the environment and skip a shell.
pub fn ensure_login_path() {
    INITIALIZED.get_or_init(|| {
        let _refresh = REFRESH_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let baseline = inherited_environment();
        let captured = if cfg!(windows)
            || (std::env::var_os("GROVE_FORCE_LOGIN_PATH").is_none() && !needs_resolution())
        {
            Some(
                baseline
                    .iter()
                    .filter_map(|(key, value)| {
                        Some((key.to_str()?.to_string(), value.to_str()?.to_string()))
                    })
                    .filter(|(key, _)| exportable_key(key))
                    .collect(),
            )
        } else {
            query_login_environment()
        };
        if let Some(env) = &captured {
            // Startup PATH still supports app-wide executable detection. Later
            // background refreshes only replace the immutable session snapshot.
            for (key, value) in env {
                std::env::set_var(key, value);
            }
        }
        let _ = SESSION_ENV
            .get_or_init(EnvironmentCache::default)
            .publish(captured);
    });
}

/// Launches keep their immutable snapshot while a refresh runs. A missing
/// snapshot retains the interactive-shell fallback after startup failure.
pub fn session_environment() -> Option<Arc<[(String, String)]>> {
    SESSION_ENV.get().and_then(EnvironmentCache::snapshot)
}

/// Capture from the original inherited environment so removing a shell-added
/// rc export removes it from future sessions. Explicit inherited exports remain
/// unless shell configuration unsets them. Failed captures keep the last snapshot.
pub fn refresh_login_environment() -> Result<(), String> {
    if cfg!(windows) {
        return Err("Shell environment refresh is unavailable on Windows.".into());
    }
    let _refresh = REFRESH_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    SESSION_ENV
        .get_or_init(EnvironmentCache::default)
        .publish(query_login_environment())
}

fn exportable_key(key: &str) -> bool {
    crate::tmux::valid_env_key(key) && !matches!(key, "PWD" | "OLDPWD" | "SHLVL" | "_")
}

const ENV_START: &[u8] = b"__GROVE_ENV_START__\0";
const ENV_END: &[u8] = b"__GROVE_ENV_END__\0";

fn extract_environment(raw: &[u8]) -> Option<Vec<(String, String)>> {
    let start = raw.windows(ENV_START.len()).position(|w| w == ENV_START)? + ENV_START.len();
    let rest = &raw[start..];
    let end = rest.windows(ENV_END.len()).position(|w| w == ENV_END)?;
    let mut env = Vec::new();
    for entry in rest[..end]
        .split(|b| *b == 0)
        .filter(|entry| !entry.is_empty())
    {
        let entry = std::str::from_utf8(entry).ok()?;
        let (key, value) = entry.split_once('=')?;
        if !exportable_key(key) {
            continue;
        }
        env.push((key.to_string(), value.to_string()));
    }
    if !env
        .iter()
        .any(|(key, value)| key == "PATH" && !value.is_empty())
    {
        return None;
    }
    Some(env)
}

/// Either signal is sufficient: no controlling terminal (the definitive GUI-launch case), or PATH still looks like a bare system default.
fn needs_resolution() -> bool {
    if cfg!(windows) {
        return false;
    }
    launched_without_terminal() || looks_thin()
}

/// Requires *both* stdin and stdout non-tty, so a terminal launch with merely-redirected output (`grove > log`) doesn't misfire.
fn launched_without_terminal() -> bool {
    use std::io::IsTerminal;
    !std::io::stdin().is_terminal() && !std::io::stdout().is_terminal()
}

fn looks_thin() -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return true;
    };

    let home = std::env::var("HOME").unwrap_or_default();
    let cargo_bin = format!("{home}/.cargo/bin");
    let local_bin = format!("{home}/.local/bin");
    // Deliberately excludes `/usr/local/bin`: macOS `path_helper` injects it into every Finder/Launchpad launch regardless, so its presence doesn't prove a real shell PATH.
    let rich_markers = [
        cargo_bin.as_str(),
        local_bin.as_str(),
        "/opt/homebrew/bin",
        "/home/linuxbrew/.linuxbrew/bin",
    ];

    let dirs: Vec<_> = std::env::split_paths(&path).collect();
    !dirs
        .iter()
        .any(|dir| rich_markers.iter().any(|m| dir.as_os_str() == *m))
}

/// Accepted only when `$SHELL` is an absolute path to an existing file; anything else falls back to `/bin/sh`. See [`windows_shell`] for Windows.
pub fn login_shell() -> String {
    #[cfg(windows)]
    {
        windows_shell()
    }
    #[cfg(not(windows))]
    {
        match std::env::var("SHELL") {
            Ok(s) if s.starts_with('/') && std::path::Path::new(&s).is_file() => s,
            _ => "/bin/sh".into(),
        }
    }
}

/// Prefers `pwsh.exe` (supports `&&`/`||` chaining) over the always-present but limited `powershell.exe` 5.1.
#[cfg(windows)]
fn windows_shell() -> String {
    if find_on_path("pwsh.exe").is_some() {
        "pwsh.exe".into()
    } else {
        "powershell.exe".into()
    }
}

/// A plain existence check, not the PATHEXT-aware search agent binaries need (see `agent::resolve_on_path`).
#[cfg(windows)]
fn find_on_path(name: &str) -> Option<std::path::PathBuf> {
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .map(|dir| dir.join(name))
        .find(|p| p.is_file())
}

/// Drain stdout concurrently so a large environment cannot fill the pipe.
/// Reap the shell process group and bound waiting for the output reader.
fn query_login_environment() -> Option<Vec<(String, String)>> {
    let shell = inherited_environment()
        .iter()
        .find(|(key, _)| key == "SHELL")
        .and_then(|(_, value)| value.to_str())
        .filter(|shell| shell.starts_with('/') && std::path::Path::new(shell).is_file())
        .unwrap_or("/bin/sh");
    query_environment(shell, Duration::from_secs(15), inherited_environment())
}

fn query_environment(
    shell: &str,
    timeout: Duration,
    baseline: &[(OsString, OsString)],
) -> Option<Vec<(String, String)>> {
    use std::io::Read;
    use std::process::Stdio;
    let script = "/bin/sh -c 'printf \"__GROVE_ENV_START__\\0\"; /usr/bin/env -0; printf \"__GROVE_ENV_END__\\0\"'";
    let mut command = Command::new(shell);
    command
        .env_clear()
        .envs(baseline.iter().cloned())
        .args(["-lic", script])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command.spawn().ok()?;
    let mut stdout = child.stdout.take()?;
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = stdout.read_to_end(&mut bytes).ok().map(|_| bytes);
        let _ = sender.send(result);
    });
    let deadline = std::time::Instant::now() + timeout;
    let success = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(50));
            }
            _ => break false,
        }
    };
    // Also terminate background rc helpers retaining the output pipe.
    #[cfg(unix)]
    {
        let _ = Command::new("/bin/kill")
            .args(["-KILL", "--", &format!("-{}", child.id())])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
    // A detached rc helper may escape the process group and hold the pipe.
    // Never let its reader delay the startup fallback.
    let raw = receiver.recv_timeout(Duration::from_millis(100)).ok()??;
    success.then(|| extract_environment(&raw)).flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refresh_atomically_replaces_exports_and_retains_good_snapshot_on_failure() {
        let cache = EnvironmentCache::default();
        let old = vec![
            ("PATH".into(), "/old".into()),
            ("REMOVED".into(), "value".into()),
        ];
        cache.publish(Some(old.clone())).unwrap();
        let held = cache.snapshot().unwrap();
        assert!(cache.publish(None).is_err());
        assert_eq!(&*cache.snapshot().unwrap(), old.as_slice());
        let fresh = vec![("PATH".into(), "/new".into())];
        cache.publish(Some(fresh.clone())).unwrap();
        assert_eq!(&*cache.snapshot().unwrap(), fresh.as_slice());
        assert_eq!(&*held, old.as_slice());
        let initially_failed = EnvironmentCache::default();
        assert!(initially_failed.publish(None).is_err());
        assert!(initially_failed.snapshot().is_none());
        initially_failed.publish(Some(fresh.clone())).unwrap();
        assert_eq!(&*initially_failed.snapshot().unwrap(), fresh.as_slice());
    }

    #[cfg(unix)]
    #[test]
    fn refresh_capture_uses_original_baseline_so_deleted_exports_disappear() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let shell = dir.path().join("shell");
        let baseline = vec![(OsString::from("PATH"), OsString::from("/usr/bin:/bin"))];
        fs_err::write(
            &shell,
            b"#!/bin/sh\nexport GROVE_TEST_REMOVED=value\nexec /bin/sh -c \"$2\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o700)).unwrap();
        let first =
            query_environment(shell.to_str().unwrap(), Duration::from_secs(2), &baseline).unwrap();
        assert!(first.iter().any(|(key, _)| key == "GROVE_TEST_REMOVED"));
        fs_err::write(&shell, b"#!/bin/sh\nexec /bin/sh -c \"$2\"\n").unwrap();
        let second =
            query_environment(shell.to_str().unwrap(), Duration::from_secs(2), &baseline).unwrap();
        assert!(!second.iter().any(|(key, _)| key == "GROVE_TEST_REMOVED"));
    }

    #[test]
    fn extracts_exported_environment_amid_banner_noise() {
        let raw = b"banner\n__GROVE_ENV_START__\0PATH=/bin:/usr/bin\0API_TOKEN=spaces; $(literal)\nsecond=line\0__GROVE_ENV_END__\0prompt";
        assert_eq!(
            extract_environment(raw),
            Some(vec![
                ("PATH".into(), "/bin:/usr/bin".into()),
                ("API_TOKEN".into(), "spaces; $(literal)\nsecond=line".into())
            ])
        );
    }

    #[test]
    fn environment_requires_fences_and_nonempty_path() {
        assert!(extract_environment(b"PATH=/bin\0").is_none());
        assert!(extract_environment(b"__GROVE_ENV_START__\0PATH=\0__GROVE_ENV_END__\0").is_none());
    }

    #[cfg(unix)]
    #[test]
    fn shell_capture_preserves_exports_and_bounds_hanging_initialization() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let shell = dir.path().join("shell");
        fs_err::write(
            &shell,
            b"#!/bin/sh\nexport GROVE_TEST_EXPORT='spaces; $(literal)'\nexec /bin/sh -c \"$2\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o700)).unwrap();
        let env = query_environment(
            shell.to_str().unwrap(),
            Duration::from_secs(2),
            &std::env::vars_os().collect::<Vec<_>>(),
        )
        .unwrap();
        assert!(env.contains(&("GROVE_TEST_EXPORT".into(), "spaces; $(literal)".into())));
        assert!(!env
            .iter()
            .any(|(key, _)| matches!(key.as_str(), "PWD" | "OLDPWD" | "SHLVL" | "_")));
        fs_err::write(&shell, b"#!/bin/sh\n/bin/sleep 2\n").unwrap();
        let start = std::time::Instant::now();
        assert!(query_environment(
            shell.to_str().unwrap(),
            Duration::from_millis(100),
            &std::env::vars_os().collect::<Vec<_>>()
        )
        .is_none());
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    #[cfg(unix)]
    #[test]
    fn shell_can_explicitly_unset_exports_in_the_original_inherited_baseline() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let shell = dir.path().join("shell");
        fs_err::write(
            &shell,
            b"#!/bin/sh\nunset GROVE_TEST_INHERITED\nexec /bin/sh -c \"$2\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o700)).unwrap();
        let baseline = vec![
            (OsString::from("PATH"), OsString::from("/usr/bin:/bin")),
            (
                OsString::from("GROVE_TEST_INHERITED"),
                OsString::from("original"),
            ),
        ];
        let env =
            query_environment(shell.to_str().unwrap(), Duration::from_secs(2), &baseline).unwrap();
        assert!(!env.iter().any(|(key, _)| key == "GROVE_TEST_INHERITED"));
    }

    // `set_var`/`remove_var` are not thread-safe, so both cases are combined into one test function to run sequentially.
    #[test]
    fn login_shell_absolute_existing_vs_fallback() {
        std::env::set_var("SHELL", "/bin/sh");
        let shell = login_shell();
        assert_eq!(
            shell, "/bin/sh",
            "login_shell must return /bin/sh when $SHELL=/bin/sh"
        );

        std::env::set_var("SHELL", "bash");
        let shell = login_shell();
        assert_eq!(
            shell, "/bin/sh",
            "login_shell must return /bin/sh when $SHELL is a relative path"
        );

        std::env::set_var("SHELL", "/does/not/exist/myshell");
        let shell = login_shell();
        assert_eq!(
            shell, "/bin/sh",
            "login_shell must return /bin/sh when $SHELL points to a nonexistent file"
        );

        // Best-effort restore so other tests aren't affected.
        if let Ok(real) = std::env::var("SHELL") {
            if real == "/does/not/exist/myshell" {
                std::env::set_var("SHELL", "/bin/sh");
            }
        }
    }

    // Windows-only; mutates the process-global PATH, so serialized behind a mutex like `theme.rs`'s `CUSTOM_TEST_LOCK`.
    #[cfg(windows)]
    static WINDOWS_SHELL_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[cfg(windows)]
    #[test]
    fn windows_shell_prefers_pwsh_when_present() {
        use fs_err as fs;
        let _lock = WINDOWS_SHELL_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("pwsh.exe"), b"").unwrap();
        fs::write(dir.path().join("powershell.exe"), b"").unwrap();

        let old_path = std::env::var_os("PATH");
        std::env::set_var("PATH", dir.path());

        assert_eq!(windows_shell(), "pwsh.exe");

        if let Some(p) = old_path {
            std::env::set_var("PATH", p);
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_shell_falls_back_to_powershell_without_pwsh() {
        use fs_err as fs;
        let _lock = WINDOWS_SHELL_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("powershell.exe"), b"").unwrap();

        let old_path = std::env::var_os("PATH");
        std::env::set_var("PATH", dir.path());

        assert_eq!(windows_shell(), "powershell.exe");

        if let Some(p) = old_path {
            std::env::set_var("PATH", p);
        }
    }
}
