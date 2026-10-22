#[cfg(any(unix, test))]
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
                args.push("--no-alt-screen".into());
                args.push("-c".into());
                args.push(r#"tui.terminal_title=["activity","thread-title"]"#.into());
                if skip_permissions {
                    args.push("--dangerously-bypass-approvals-and-sandbox".into());
                }
            }
            Agent::OpenCode | Agent::Terminal => {}
        }
        args
    }

    /// Global permission preferences belong to CLI agents only. Explicit native
    /// caller options are passed separately and must not use this preference.
    pub fn launch_args_for_backend(
        self,
        native: bool,
        skip_permissions: bool,
        chrome: bool,
    ) -> Vec<String> {
        self.launch_args(!native && skip_permissions, chrome)
    }

    pub fn multi_root_launch_args_for_backend(
        self,
        native: bool,
        skip_permissions: bool,
        chrome: bool,
        additional_roots: &[String],
    ) -> Option<Vec<String>> {
        self.multi_root_launch_args(!native && skip_permissions, chrome, additional_roots)
    }

    /// Builds launch arguments for one primary root plus additional writable roots.
    /// Claude and Codex accept repeated `--add-dir <path>` pairs. OpenCode and
    /// Terminal receive their extra roots through Grove's symlink bundle.
    pub fn multi_root_launch_args(
        self,
        skip_permissions: bool,
        chrome: bool,
        additional_roots: &[String],
    ) -> Option<Vec<String>> {
        match self {
            Agent::Claude | Agent::Codex => {
                let mut args = self.launch_args(skip_permissions, chrome);
                for root in additional_roots {
                    args.push("--add-dir".into());
                    args.push(root.clone());
                }
                Some(args)
            }
            Agent::OpenCode | Agent::Terminal => Some(self.launch_args(skip_permissions, chrome)),
        }
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
                    (
                        resolve_unix_agent(self.binary_name()).map_or_else(
                            || self.binary_name().to_string(),
                            |p| p.to_string_lossy().into_owned(),
                        ),
                        vec![],
                    )
                }
            }
        }
    }

    /// Builds the argv used when launching an agent session. On Unix, run the
    /// agent directly after startup captured the exported shell environment.
    /// If capture failed, retain the fresh interactive login shell fallback.
    /// Arguments always remain separate argv elements.
    pub fn session_invocation(self, args: &[String]) -> (String, Vec<String>) {
        self.session_invocation_with_environment(
            args,
            crate::env_path::session_environment().is_some(),
        )
    }

    fn session_invocation_with_environment(
        self,
        args: &[String],
        resolved: bool,
    ) -> (String, Vec<String>) {
        let (program, prefix_args) = self.invocation();

        if matches!(self, Agent::Terminal) || resolved {
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
            (crate::env_path::login_shell(), shell_args)
        }

        #[cfg(windows)]
        {
            let mut invocation_args = prefix_args;
            invocation_args.extend(args.iter().cloned());
            (program, invocation_args)
        }
    }

    /// Cached by the immutable session environment, so a successful shell refresh
    /// invalidates availability without filesystem work on every render.
    pub fn available(self) -> bool {
        type Cache = Option<(
            Option<crate::env_path::Environment>,
            Option<std::ffi::OsString>,
            [bool; 3],
        )>;
        static CACHE: OnceLock<std::sync::Mutex<Cache>> = OnceLock::new();
        if self == Agent::Terminal {
            return true;
        }
        let environment = crate::env_path::session_environment();
        let path = std::env::var_os("PATH");
        let mut cache = CACHE
            .get_or_init(|| std::sync::Mutex::new(None))
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let same_environment =
            |old: &Option<crate::env_path::Environment>| match (old, &environment) {
                (Some(old), Some(new)) => std::sync::Arc::ptr_eq(old, new),
                (None, None) => true,
                _ => false,
            };
        if cache
            .as_ref()
            .is_none_or(|(old_env, old_path, _)| !same_environment(old_env) || *old_path != path)
        {
            *cache = Some((
                environment,
                path,
                [Agent::Claude, Agent::Codex, Agent::OpenCode].map(Agent::detect),
            ));
        }
        let index = match self {
            Agent::Claude => 0,
            Agent::Codex => 1,
            Agent::OpenCode => 2,
            Agent::Terminal => return true,
        };
        cache
            .as_ref()
            .is_some_and(|(_, _, available)| available[index])
    }

    /// Uncached; called only when the session environment changes.
    fn detect(self) -> bool {
        let name = self.binary_name();
        #[cfg(windows)]
        {
            resolve_on_path(name).is_some()
        }
        #[cfg(not(windows))]
        {
            resolve_unix_agent(name).is_some()
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

/// Startup already captures the login shell environment. Also search standard user
/// install locations when Finder's inherited PATH or a shell initialization failure
/// omitted them. This same resolver serves availability and actual process launch.
#[cfg(not(windows))]
fn resolve_unix_agent(name: &str) -> Option<std::path::PathBuf> {
    let inherited = std::env::var_os("PATH").unwrap_or_default();
    let captured = crate::env_path::session_environment();
    let path = captured
        .as_ref()
        .and_then(|env| env.iter().find(|(k, _)| k == "PATH"))
        .map_or(inherited, |(_, v)| std::ffi::OsString::from(v));
    resolve_unix_agent_in(name, &path, std::env::var_os("HOME").as_deref())
}
#[cfg(not(windows))]
fn resolve_unix_agent_in(
    name: &str,
    path: &std::ffi::OsStr,
    home: Option<&std::ffi::OsStr>,
) -> Option<std::path::PathBuf> {
    let mut dirs: Vec<_> = std::env::split_paths(path).collect();
    if let Some(home) = home {
        let home = std::path::Path::new(home);
        dirs.extend(
            [
                ".npm-global/bin",
                ".local/bin",
                ".cargo/bin",
                ".bun/bin",
                ".volta/bin",
            ]
            .map(|dir| home.join(dir)),
        );
    }
    dirs.extend(["/opt/homebrew/bin", "/usr/local/bin"].map(std::path::PathBuf::from));
    dirs.into_iter()
        .map(|dir| dir.join(name))
        .find(|path| is_executable(path.clone()))
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
    #[test]
    fn cli_access_preference_is_absent_from_native_launches_with_roots_and_chrome() {
        let roots = vec!["/extra".to_owned()];
        for agent in [Agent::Claude, Agent::Codex] {
            assert_eq!(
                agent.launch_args_for_backend(true, true, true),
                agent.launch_args(false, true)
            );
            assert_eq!(
                agent.multi_root_launch_args_for_backend(true, true, true, &roots),
                agent.multi_root_launch_args(false, true, &roots)
            );
            assert_eq!(
                agent.launch_args_for_backend(false, true, true),
                agent.launch_args(true, true)
            );
            assert_eq!(
                agent.multi_root_launch_args_for_backend(false, true, true, &roots),
                agent.multi_root_launch_args(true, true, &roots)
            );
        }
    }
    #[cfg(unix)]
    #[test]
    fn finder_path_resolves_standard_user_install_and_preserves_path_priority() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let install = home.path().join(".npm-global/bin");
        fs_err::create_dir_all(&install).unwrap();
        let program = install.join("codex");
        fs_err::write(&program, "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(
            super::resolve_unix_agent_in(
                "codex",
                std::ffi::OsStr::new("/usr/bin:/bin:/usr/sbin:/sbin"),
                Some(home.path().as_os_str())
            ),
            Some(program)
        );
        let preferred = home.path().join("configured");
        fs_err::create_dir_all(&preferred).unwrap();
        let program = preferred.join("codex");
        fs_err::write(&program, "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(
            super::resolve_unix_agent_in(
                "codex",
                preferred.as_os_str(),
                Some(home.path().as_os_str())
            ),
            Some(program)
        );
    }

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

    #[test]
    fn multi_root_args_append_one_native_pair_per_extra_root() {
        let roots = vec!["/worktrees/second".into(), "/worktrees/third".into()];
        assert_eq!(
            Agent::Claude.multi_root_launch_args(false, false, &roots),
            Some(vec![
                "--add-dir".into(),
                "/worktrees/second".into(),
                "--add-dir".into(),
                "/worktrees/third".into(),
            ])
        );
        assert_eq!(
            Agent::Codex.multi_root_launch_args(true, false, &roots),
            Some(vec![
                "--no-alt-screen".into(),
                "-c".into(),
                r#"tui.terminal_title=["activity","thread-title"]"#.into(),
                "--dangerously-bypass-approvals-and-sandbox".into(),
                "--add-dir".into(),
                "/worktrees/second".into(),
                "--add-dir".into(),
                "/worktrees/third".into(),
            ])
        );
    }

    #[test]
    fn bundle_agents_accept_multi_root_launches_without_native_flags() {
        let roots = vec!["/worktrees/second".into()];
        assert_eq!(
            Agent::OpenCode.multi_root_launch_args(false, false, &roots),
            Some(Vec::new())
        );
        assert_eq!(
            Agent::Terminal.multi_root_launch_args(false, false, &roots),
            Some(Vec::new())
        );
    }

    /// U3: `--chrome` is Claude-only; no other agent ever gets it.
    #[test]
    fn chrome_flag_is_claude_only() {
        for agent in [Agent::OpenCode, Agent::Terminal] {
            assert!(agent.launch_args(false, true).is_empty());
        }
        assert_eq!(
            Agent::Codex.launch_args(false, true),
            vec![
                "--no-alt-screen".to_string(),
                "-c".to_string(),
                r#"tui.terminal_title=["activity","thread-title"]"#.to_string(),
            ]
        );
    }

    #[test]
    fn codex_launch_preserves_scrollback_and_requests_generated_thread_titles() {
        assert_eq!(
            Agent::Codex.launch_args(false, false),
            vec![
                "--no-alt-screen".to_string(),
                "-c".to_string(),
                r#"tui.terminal_title=["activity","thread-title"]"#.to_string(),
            ]
        );
        assert_eq!(
            Agent::Codex.launch_args(true, false),
            vec![
                "--no-alt-screen".to_string(),
                "-c".to_string(),
                r#"tui.terminal_title=["activity","thread-title"]"#.to_string(),
                "--dangerously-bypass-approvals-and-sandbox".to_string(),
            ]
        );
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
    fn invocation_uses_shared_resolution_or_bare_name_when_unresolved() {
        // On Windows with no PATH match, falls back to the bare name to preserve the "not found" UX.
        let (program, prefix_args) = Agent::Claude.invocation();
        assert!(prefix_args.is_empty() || cfg!(windows));
        #[cfg(not(windows))]
        assert_eq!(
            program,
            super::resolve_unix_agent("claude")
                .map_or_else(|| "claude".into(), |p| p.to_string_lossy().into_owned())
        );
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
            [
                Agent::Codex.invocation().0,
                args[0].clone(),
                args[1].clone()
            ]
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn resolved_environment_launches_agents_directly_with_literal_arguments() {
        let args = vec![
            "spaces; $(literal)".to_string(),
            "--add-dir=/tmp/two roots".to_string(),
        ];
        for agent in [Agent::Claude, Agent::Codex, Agent::OpenCode] {
            let (program, argv) = agent.session_invocation_with_environment(&args, true);
            assert_eq!(program, agent.invocation().0);
            assert_eq!(argv, args);
        }
    }

    #[test]
    fn terminal_session_invocation_remains_plain_shell() {
        let args = vec!["--existing-terminal-arg".to_string()];
        let (program, invocation_args) = Agent::Terminal.session_invocation(&args);

        assert_eq!(program, Agent::Terminal.invocation().0);
        assert_eq!(invocation_args, args);
    }
}
