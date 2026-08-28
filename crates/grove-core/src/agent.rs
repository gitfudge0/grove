use fs_err as fs;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Agent {
    Claude,
    Codex,
    OpenCode,
    Terminal,
}

impl Agent {
    pub const ALL: [Agent; 4] = [
        Agent::Claude,
        Agent::Codex,
        Agent::OpenCode,
        Agent::Terminal,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Agent::Claude => "claude",
            Agent::Codex => "codex",
            Agent::OpenCode => "opencode",
            Agent::Terminal => "terminal",
        }
    }

    /// The inline SVG sprite name (see `gui::icons`); `Terminal` reuses the generic terminal glyph.
    pub fn icon_name(self) -> &'static str {
        match self {
            Agent::Claude => "claude",
            Agent::Codex => "codex",
            Agent::OpenCode => "opencode",
            Agent::Terminal => "term",
        }
    }

    pub fn program(self) -> String {
        match self {
            Agent::Claude | Agent::Codex | Agent::OpenCode => self.binary_name().into(),
            // The user's login shell (validated), falling back to a POSIX shell.
            Agent::Terminal => crate::env_path::login_shell(),
        }
    }

    /// `Terminal` has no static name (resolved via `$SHELL`) — callers must guard against it.
    fn binary_name(self) -> &'static str {
        match self {
            Agent::Claude => "claude",
            Agent::Codex => "codex",
            Agent::OpenCode => "opencode",
            Agent::Terminal => "",
        }
    }

    /// Flags accumulate in order (skip-permissions, then `--chrome`); only Claude supports `--chrome`.
    pub fn launch_args(self, skip_permissions: bool, chrome: bool) -> Vec<String> {
        let mut args: Vec<String> = Vec::new();
        match self {
            Agent::Claude => {
                if skip_permissions {
                    args.push("--dangerously-skip-permissions".into());
                }
                if chrome {
                    args.push("--chrome".into());
                }
            }
            Agent::Codex => {
                if skip_permissions {
                    args.push("--dangerously-bypass-approvals-and-sandbox".into());
                }
            }
            Agent::OpenCode | Agent::Terminal => {}
        }
        args
    }

    /// On Windows, a resolved `.cmd`/`.bat` shim is wrapped as `cmd.exe /C <path>` since `CreateProcess` can't exec it directly; `.exe` and Unix run the binary directly.
    pub fn invocation(self) -> (String, Vec<String>) {
        match self {
            Agent::Terminal => (self.program(), vec![]),
            Agent::Claude | Agent::Codex | Agent::OpenCode => {
                #[cfg(windows)]
                {
                    let name = self.binary_name();
                    if let Some(path) = resolve_on_path(name) {
                        let is_script = path
                            .extension()
                            .and_then(|e| e.to_str())
                            .map(|e| e.eq_ignore_ascii_case("cmd") || e.eq_ignore_ascii_case("bat"))
                            .unwrap_or(false);
                        if is_script {
                            return (
                                "cmd.exe".into(),
                                vec!["/C".into(), path.display().to_string()],
                            );
                        }
                        return (path.display().to_string(), vec![]);
                    }
                    (name.to_string(), vec![])
                }
                #[cfg(not(windows))]
                {
                    (self.binary_name().to_string(), vec![])
                }
            }
        }
    }

    /// Builds the argv used when launching an agent session. On Unix, run the
    /// agent through a fresh interactive login shell so current shell rc
    /// changes are loaded at spawn time. The agent and all of its arguments
    /// remain separate argv elements behind `--`.
    pub fn session_invocation(self, args: &[String]) -> (String, Vec<String>) {
        let (program, prefix_args) = self.invocation();

        if matches!(self, Agent::Terminal) {
            let mut invocation_args = prefix_args;
            invocation_args.extend(args.iter().cloned());
            return (program, invocation_args);
        }

        #[cfg(not(windows))]
        {
            let mut shell_args = vec![
                "-lic".to_string(),
                "exec \"$@\"".to_string(),
                "--".to_string(),
                program,
            ];
            shell_args.extend(prefix_args);
            shell_args.extend(args.iter().cloned());
            return (crate::env_path::login_shell(), shell_args);
        }

        #[cfg(windows)]
        {
            let mut invocation_args = prefix_args;
            invocation_args.extend(args.iter().cloned());
            (program, invocation_args)
        }
    }

    /// Cached for the process lifetime, since this runs on `view()`'s render path; a CLI installed after startup won't be noticed until restart.
    pub fn available(self) -> bool {
        match self {
            Agent::Terminal => true,
            Agent::Claude | Agent::Codex | Agent::OpenCode => {
                static CACHE: OnceLock<[bool; 3]> = OnceLock::new();
                let cached = CACHE.get_or_init(|| {
                    [Agent::Claude, Agent::Codex, Agent::OpenCode].map(Agent::detect)
                });
                match self {
                    Agent::Claude => cached[0],
                    Agent::Codex => cached[1],
                    Agent::OpenCode => cached[2],
                    Agent::Terminal => unreachable!(),
                }
            }
        }
    }

    /// Uncached; called once per variant from inside `available()`'s `OnceLock`.
    fn detect(self) -> bool {
        let name = self.binary_name();
        #[cfg(windows)]
        {
            resolve_on_path(name).is_some()
        }
        #[cfg(not(windows))]
        {
            std::env::var_os("PATH").is_some_and(|paths| {
                std::env::split_paths(&paths).any(|dir| is_executable(dir.join(name)))
            })
        }
    }

    /// Returns the trimmed first stdout line, or `None` for `Terminal`/spawn failure; callers should run this off the UI thread.
    pub fn version(self) -> Option<String> {
        if matches!(self, Agent::Terminal) {
            return None;
        }
        if self.binary_name().is_empty() {
            return None;
        }
        let (program, prefix_args) = self.invocation();
        let output = std::process::Command::new(&program)
            .args(&prefix_args)
            .arg("--version")
            .stdin(std::process::Stdio::null())
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .map(str::to_string)
    }
}

/// A regular file with an execute bit set; falls back to `is_file()` on non-Unix targets.
#[cfg(not(windows))]
fn is_executable(path: std::path::PathBuf) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(&path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

/// Windows-only: searches `$PATH` × `%PATHEXT%` in order, mirroring how `cmd.exe` resolves a bare command name.
#[cfg(windows)]
fn resolve_on_path(name: &str) -> Option<std::path::PathBuf> {
    let paths = std::env::var_os("PATH")?;
    let pathext = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
    let exts: Vec<&str> = pathext
        .split(';')
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .collect();

    for dir in std::env::split_paths(&paths) {
        for ext in &exts {
            let candidate = dir.join(format!("{name}{ext}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// U1: the chrome toggle alone yields exactly `--chrome` for Claude.
    #[test]
    fn claude_chrome_only() {
        assert_eq!(Agent::Claude.launch_args(false, true), vec!["--chrome"]);
    }

    /// U2: both toggles accumulate, in order.
    #[test]
    fn claude_both_flags_accumulate_in_order() {
        let args = Agent::Claude.launch_args(true, true);
        assert_eq!(args.len(), 2);
        assert_eq!(args, vec!["--dangerously-skip-permissions", "--chrome"]);
    }

    /// U3: `--chrome` is Claude-only; no other agent ever gets it.
    #[test]
    fn chrome_flag_is_claude_only() {
        for agent in [Agent::Codex, Agent::OpenCode, Agent::Terminal] {
            assert!(agent.launch_args(false, true).is_empty());
        }
    }

    #[cfg(windows)]
    #[test]
    fn resolve_on_path_finds_cmd_shim() {
        let dir = std::env::temp_dir().join("grove_test_agent_cmd_shim");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("claude.cmd"), b"").unwrap();

        let old_path = std::env::var_os("PATH");
        std::env::set_var("PATH", &dir);
        std::env::set_var("PATHEXT", ".COM;.EXE;.BAT;.CMD");

        let resolved = resolve_on_path("claude").expect("should find claude.cmd");
        assert_eq!(resolved.file_name().unwrap(), "claude.cmd");

        if let Some(p) = old_path {
            std::env::set_var("PATH", p);
        }
        fs::remove_dir_all(&dir).ok();
    }

    #[cfg(windows)]
    #[test]
    fn resolve_on_path_prefers_exe_order_in_pathext() {
        let dir = std::env::temp_dir().join("grove_test_agent_exe_priority");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("codex.cmd"), b"").unwrap();
        fs::write(dir.join("codex.exe"), b"").unwrap();

        let old_path = std::env::var_os("PATH");
        std::env::set_var("PATH", &dir);
        // .EXE listed before .CMD: resolve_on_path must return codex.exe.
        std::env::set_var("PATHEXT", ".COM;.EXE;.BAT;.CMD");

        let resolved = resolve_on_path("codex").expect("should find a match");
        assert_eq!(resolved.file_name().unwrap(), "codex.exe");

        if let Some(p) = old_path {
            std::env::set_var("PATH", p);
        }
        fs::remove_dir_all(&dir).ok();
    }

    #[cfg(windows)]
    #[test]
    fn invocation_wraps_cmd_shim_with_cmd_exe() {
        let dir = std::env::temp_dir().join("grove_test_agent_invocation");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("opencode.cmd"), b"").unwrap();

        let old_path = std::env::var_os("PATH");
        std::env::set_var("PATH", &dir);
        std::env::set_var("PATHEXT", ".COM;.EXE;.BAT;.CMD");

        let (program, prefix_args) = Agent::OpenCode.invocation();
        assert_eq!(program, "cmd.exe");
        assert_eq!(prefix_args.len(), 2);
        assert_eq!(prefix_args[0], "/C");
        assert!(prefix_args[1].ends_with("opencode.cmd"));

        if let Some(p) = old_path {
            std::env::set_var("PATH", p);
        }
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn invocation_is_plain_binary_name_on_non_windows_or_when_unresolved() {
        // On Windows with no PATH match, falls back to the bare name to preserve the "not found" UX.
        let (program, prefix_args) = Agent::Claude.invocation();
        assert!(prefix_args.is_empty() || cfg!(windows));
        #[cfg(not(windows))]
        assert_eq!(program, "claude");
    }

    #[cfg(not(windows))]
    #[test]
    fn session_invocation_sources_login_shell_without_interpolating_args() {
        let args = vec![
            "--prompt=hello; touch unsafe".to_string(),
            "$(echo nope)".to_string(),
        ];
        let (program, invocation_args) = Agent::Codex.session_invocation(&args);

        assert_eq!(program, crate::env_path::login_shell());
        assert_eq!(&invocation_args[..3], ["-lic", "exec \"$@\"", "--"]);
        assert_eq!(
            &invocation_args[3..],
            ["codex".to_string(), args[0].clone(), args[1].clone()]
        );
    }

    #[test]
    fn terminal_session_invocation_remains_plain_shell() {
        let args = vec!["--existing-terminal-arg".to_string()];
        let (program, invocation_args) = Agent::Terminal.session_invocation(&args);

        assert_eq!(program, Agent::Terminal.invocation().0);
        assert_eq!(invocation_args, args);
    }
}
