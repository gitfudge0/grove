//! Login-PATH resolution for GUI launches.
//!
//! When Grove is launched from a terminal it inherits the user's full `$PATH`
//! and everything just works. When it's launched from Finder/Launchpad (macOS)
//! or an application menu (`.desktop` on Linux), the OS hands the process a
//! minimal environment (typically `/usr/bin:/bin`). That breaks Grove's two
//! external dependencies: it can't find `claude`/`codex`/`opencode` on `$PATH`
//! (see `crate::agent::Agent::available`) and the shells it spawns start
//! without the user's tooling.
//!
//! [`ensure_login_path`] detects a "thin" PATH and, when found, asks the user's
//! login shell for the real PATH and installs it into this process's
//! environment before any PTYs are spawned.

use std::process::Command;
use std::time::Duration;

/// If the current `$PATH` looks like a stripped-down GUI-launch environment,
/// query the user's login shell for the real PATH and set it on this process.
///
/// No-op when the PATH already looks rich (the common terminal-launch case) so
/// running `grove` from a shell is completely unaffected. Set
/// `GROVE_FORCE_LOGIN_PATH=1` to force resolution regardless of the heuristic.
pub fn ensure_login_path() {
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

/// Markers wrapping the PATH we print from the login shell. An interactive
/// shell (`-i`) sources rc files that may emit banners, prompts, or other noise
/// to stdout (e.g. fastfetch/neofetch art, MOTDs). Fencing the real value lets
/// us extract just the PATH and discard everything around it — otherwise that
/// noise gets spliced into `$PATH`, producing absurdly long "directory" entries
/// that make later `Command` spawns fail with `ENAMETOOLONG` (os error 63).
const PATH_START: &str = "__GROVE_PATH_START__";
const PATH_END: &str = "__GROVE_PATH_END__";

/// Pull the fenced PATH out of raw login-shell stdout, ignoring any surrounding
/// banner/prompt output. Returns `None` if the markers aren't both present.
fn extract_fenced_path(raw: &str) -> Option<&str> {
    let start = raw.find(PATH_START)? + PATH_START.len();
    let rest = &raw[start..];
    let end = rest.find(PATH_END)?;
    Some(&rest[..end])
}

/// Should we import the login shell's PATH? Two independent signals, either is
/// sufficient:
///
///   1. We were launched without a controlling terminal — the definitive
///      GUI-launch case (Finder/Launchpad on macOS, a `.desktop` entry on
///      Linux). This is content-free and OS-agnostic, so it sidesteps the
///      trap that per-OS "default PATH" heuristics fall into.
///   2. PATH still looks like a bare system default. Belt-and-suspenders for
///      odd launchers that hand us a tty but a stripped PATH.
fn needs_resolution() -> bool {
    launched_without_terminal() || looks_thin()
}

/// True when neither stdin nor stdout is a terminal. A shell launch wires the
/// terminal's ttys into our stdio; GUI launchers (launchd, a display manager,
/// `.desktop`) do not. Requiring *both* to be non-tty avoids misfiring on a
/// terminal launch whose output was merely redirected (e.g. `grove > log`).
fn launched_without_terminal() -> bool {
    use std::io::IsTerminal;
    !std::io::stdin().is_terminal() && !std::io::stdout().is_terminal()
}

/// Heuristic: does `$PATH` look like a minimal GUI-launch environment? We
/// consider it thin when PATH is unset/empty or it contains none of the common
/// user/tool directories where `claude` & friends typically live.
fn looks_thin() -> bool {
    let path = match std::env::var_os("PATH") {
        Some(p) => p,
        None => return true,
    };

    let home = std::env::var("HOME").unwrap_or_default();
    let cargo_bin = format!("{home}/.cargo/bin");
    let local_bin = format!("{home}/.local/bin");
    // Markers that only land on PATH once a login shell has sourced the user's
    // profile. Deliberately *excludes* `/usr/local/bin`: macOS `path_helper`
    // injects it into every Finder/Launchpad launch (it's in `/etc/paths`), so
    // its presence does NOT prove a real shell PATH. Treating it as "rich" made
    // GUI launches skip resolution, leaving tools in `~/.local/bin` (e.g.
    // `claude`) unreachable while `/usr/local/bin` tools (e.g. `codex`) worked.
    let rich_markers = [
        cargo_bin.as_str(),
        local_bin.as_str(),
        "/opt/homebrew/bin",              // Homebrew (macOS, Apple Silicon)
        "/home/linuxbrew/.linuxbrew/bin", // Homebrew (Linux)
    ];

    let dirs: Vec<_> = std::env::split_paths(&path).collect();
    !dirs
        .iter()
        .any(|dir| rich_markers.iter().any(|m| dir.as_os_str() == *m))
}

/// The user's login shell from `$SHELL`, accepted only when it's an absolute
/// path to an existing file. Anything else (relative path, stale entry, a
/// value injected by a hostile parent environment) falls back to `/bin/sh`.
pub fn login_shell() -> String {
    match std::env::var("SHELL") {
        Ok(s) if s.starts_with('/') && std::path::Path::new(&s).is_file() => s,
        _ => "/bin/sh".into(),
    }
}

/// Spawn the user's login shell and capture the PATH it produces. Returns
/// `None` on any failure (no shell, non-zero exit, timeout, bad UTF-8) so the
/// caller silently keeps the existing PATH.
fn query_login_path() -> Option<String> {
    let shell = login_shell();

    // `-l` makes it a login shell (sources profile files), `-i` interactive
    // (sources rc files where many users set PATH), `-c` runs the command. We
    // fence the PATH with unique markers so any banner/prompt output an
    // interactive shell prints to stdout can be stripped out (see
    // `extract_fenced_path`).
    //
    // We hand the actual formatting off to `/bin/sh` rather than printing
    // `"$PATH"` in the login shell directly: fish (and nushell) store `$PATH`
    // as a *list* whose quoted expansion is space-joined, which would corrupt
    // the value. The exported `PATH` env var the child `sh` inherits is always
    // colon-separated per POSIX, so this stays correct regardless of which
    // shell the user runs. bash/zsh are unaffected by the extra hop.
    let script = format!("/bin/sh -c 'printf \"{PATH_START}%s{PATH_END}\" \"$PATH\"'");
    let mut child = Command::new(&shell)
        .args(["-lic", &script])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;

    // Guard against a shell that hangs (e.g. a misbehaving rc file) by polling
    // for a short window before giving up.
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

    // ── login_shell ──────────────────────────────────────────────────────────
    //
    // IMPORTANT: `set_var`/`remove_var` are not thread-safe.  These two tests
    // are combined into a single test function so they share the same thread and
    // run sequentially, eliminating the race that would occur if they ran in
    // parallel.
    #[test]
    fn login_shell_absolute_existing_vs_fallback() {
        // Case 1: $SHELL points to a real absolute executable → returned as-is.
        // `/bin/sh` is universally present on macOS and Linux.
        std::env::set_var("SHELL", "/bin/sh");
        let shell = login_shell();
        assert_eq!(
            shell, "/bin/sh",
            "login_shell must return /bin/sh when $SHELL=/bin/sh"
        );

        // Case 2: $SHELL is relative (no leading `/`) → must fall back to /bin/sh.
        std::env::set_var("SHELL", "bash");
        let shell = login_shell();
        assert_eq!(
            shell, "/bin/sh",
            "login_shell must return /bin/sh when $SHELL is a relative path"
        );

        // Case 3: $SHELL is absolute but does not exist → must fall back to /bin/sh.
        std::env::set_var("SHELL", "/does/not/exist/myshell");
        let shell = login_shell();
        assert_eq!(
            shell, "/bin/sh",
            "login_shell must return /bin/sh when $SHELL points to a nonexistent file"
        );

        // Restore to the actual shell so other tests / the process aren't
        // affected.  Best-effort: if we can't restore we leave it as /bin/sh
        // which is a safe fallback.
        if let Ok(real) = std::env::var("SHELL") {
            if real == "/does/not/exist/myshell" {
                // We're still holding our fake value — restore to /bin/sh at
                // minimum.
                std::env::set_var("SHELL", "/bin/sh");
            }
        }
    }
}
