//! Login-PATH resolution for GUI launches: Finder/Launchpad/`.desktop` hand the process a thin PATH (e.g. `/usr/bin:/bin`), breaking agent lookup and spawned shells.
//! [`ensure_login_path`] detects a thin PATH and installs the login shell's real one before any PTYs spawn.

use std::process::Command;
use std::time::Duration;

/// No-op when PATH looks rich already, or always on Windows (PATH comes from the registry, not a login shell). Set `GROVE_FORCE_LOGIN_PATH=1` to force resolution.
pub fn ensure_login_path() {
    if cfg!(windows) {
        return;
    }
    let force = std::env::var_os("GROVE_FORCE_LOGIN_PATH").is_some();
    if !force && !needs_resolution() {
        return;
    }

    if let Some(path) = query_login_path() {
        let path = path.trim();
        if !path.is_empty() {
            std::env::set_var("PATH", path);
        }
    }
}

/// Fences the real PATH value so an interactive shell's rc-file banner/prompt noise can't splice into `$PATH` and cause `ENAMETOOLONG` on later spawns.
const PATH_START: &str = "__GROVE_PATH_START__";
const PATH_END: &str = "__GROVE_PATH_END__";

/// `None` if the markers aren't both present.
fn extract_fenced_path(raw: &str) -> Option<&str> {
    let start = raw.find(PATH_START)? + PATH_START.len();
    let rest = &raw[start..];
    let end = rest.find(PATH_END)?;
    Some(&rest[..end])
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

/// `None` on any failure (no shell, non-zero exit, timeout, bad UTF-8), so the caller silently keeps the existing PATH.
fn query_login_path() -> Option<String> {
    let shell = login_shell();

    // Formatting is handed off to `/bin/sh` rather than printed directly in the login shell: fish/nushell store `$PATH` as a space-joined list, which would corrupt the value; the exported env var a child `sh` inherits is always colon-separated.
    let script = format!("/bin/sh -c 'printf \"{PATH_START}%s{PATH_END}\" \"$PATH\"'");
    let mut child = Command::new(&shell)
        .args(["-lic", &script])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;

    // Guards against a shell that hangs (e.g. a misbehaving rc file).
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(Some(_)) => return None,
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    return None;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(_) => return None,
        }
    }

    let output = child.wait_with_output().ok()?;
    let raw = String::from_utf8(output.stdout).ok()?;
    extract_fenced_path(&raw).map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_path_amid_banner_noise() {
        let raw = format!(
            "\x1b[38;2;0;0;0m▀ welcome banner\n{PATH_START}/usr/local/bin:/usr/bin:/bin{PATH_END}\n$ ",
        );
        assert_eq!(
            extract_fenced_path(&raw),
            Some("/usr/local/bin:/usr/bin:/bin")
        );
    }

    #[test]
    fn returns_none_without_markers() {
        assert_eq!(extract_fenced_path("/usr/bin:/bin"), None);
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
