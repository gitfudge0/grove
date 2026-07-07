# Windows Support (Alpha) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Grove builds, runs, and ships on Windows as an alpha release: `cargo build --release` produces a working `grove.exe`, agent sessions launch correctly via PowerShell, and CI attaches an unsigned `.msi` installer to GitHub Releases.

**Architecture:** Reuse Grove's existing native-session fallback path (already exercised on tmux-less Unix systems) unconditionally on Windows. Add small, `cfg`-gated branches for the three genuinely Windows-specific problems: shell resolution (PowerShell instead of `/bin/sh`), agent-binary resolution (PATHEXT-aware lookup + `cmd.exe /C` invocation for `.cmd`/`.bat` npm shims), and packaging (`cargo-wix` instead of `cargo-bundle`). No changes to the tmux/native branch logic itself, the keyboard-shortcut layer, or the clipboard/PTY/dialog crates — all already Windows-compatible.

**Tech Stack:** Rust (existing `grove` binary crate), `portable-pty` (ConPTY backend on Windows, already a dependency), `cargo-wix` (new, CI-only tool) for `.msi` packaging, GitHub Actions `windows-latest` runner.

## Global Constraints

- Windows support ships labeled **alpha** everywhere a user would notice it: the `.msi` product name and the release artifact filename (spec: "Windows will be an alpha release, so we'll mention 'alpha' somewhere").
- No tmux-equivalent session persistence on Windows (spec non-goal). Every Windows session uses the existing native (non-tmux) path.
- No process-tree kill on Windows in v1 (spec non-goal) — the existing non-Unix fallback (`c.kill()`, direct child only) ships unchanged.
- No code signing for the `.msi` (spec non-goal) — ships unsigned, matching the macOS ad-hoc-signing posture of "no cert available."
- Shell default on Windows: prefer `pwsh.exe`, fall back to `powershell.exe` if `pwsh` isn't on `PATH`.
- Existing Unix behavior (macOS/Linux) must not change. Every task that touches shared code must keep `cargo test` green on this (macOS) dev machine.
- Windows-only code lives behind `#[cfg(windows)]` (not `#[cfg(not(unix))]` — Grove has no other non-Unix target, but `windows` is the precise, self-documenting predicate for "this is a Windows-specific branch").

---

## File Structure

- `src/env_path.rs` — modify: Windows branch in `login_shell()`, new `windows_shell()` + `find_on_path()` helpers, Windows no-op in `needs_resolution()`.
- `src/agent.rs` — modify: new `resolve_on_path()` (Windows PATHEXT search) and `invocation()` (program + prefix-args, handling `.cmd`/`.bat` shims), `available()` and `version()` rewired to use them.
- `src/session.rs` — modify: `spawn_native()` and `spawn_script()` use `Agent::invocation()` / platform-conditional script flag instead of hardcoding `-lc`.
- `.github/workflows/release.yml` — modify: new `windows` matrix entry, Windows-specific packaging steps.
- `wix/main.wxs` — create: WiX installer template (per-user install, Start Menu shortcut, "Grove (Alpha)" product name).
- `README.md` — modify: platform badge, install section, requirements table mention Windows (alpha).

No new files beyond `wix/main.wxs`; every other change is a small addition to an existing module along an existing seam (the `use_tmux` branch, the `is_executable`/`available` pair, the `login_shell` function).

---

## Task 1: Windows shell resolution in `env_path.rs`

**Files:**
- Modify: `src/env_path.rs`
- Test: `src/env_path.rs` (inline `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: nothing new.
- Produces: `login_shell() -> String` gains Windows behavior (still same signature, used by `Agent::program()` in `src/agent.rs:43` and `Session::spawn_script` in `src/session.rs:170`). New private helpers `find_on_path(name: &str) -> Option<PathBuf>` and `windows_shell() -> String`, both `#[cfg(windows)]`.

- [ ] **Step 1: Write the failing tests**

Add to the `#[cfg(test)] mod tests` block at the bottom of `src/env_path.rs`:

```rust
    // ── Windows shell resolution ─────────────────────────────────────────────
    //
    // These only compile/run on Windows targets. On macOS/Linux dev machines
    // and in the Linux/macOS CI legs they're simply absent from the build.
    #[cfg(windows)]
    #[test]
    fn windows_shell_prefers_pwsh_when_present() {
        let dir = std::env::temp_dir().join("grove_test_pwsh_present");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("pwsh.exe"), b"").unwrap();
        std::fs::write(dir.join("powershell.exe"), b"").unwrap();

        let old_path = std::env::var_os("PATH");
        std::env::set_var("PATH", &dir);

        assert_eq!(windows_shell(), "pwsh.exe");

        if let Some(p) = old_path {
            std::env::set_var("PATH", p);
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(windows)]
    #[test]
    fn windows_shell_falls_back_to_powershell_without_pwsh() {
        let dir = std::env::temp_dir().join("grove_test_pwsh_absent");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("powershell.exe"), b"").unwrap();

        let old_path = std::env::var_os("PATH");
        std::env::set_var("PATH", &dir);

        assert_eq!(windows_shell(), "powershell.exe");

        if let Some(p) = old_path {
            std::env::set_var("PATH", p);
        }
        std::fs::remove_dir_all(&dir).ok();
    }
```

- [ ] **Step 2: Run tests to verify current state**

Run: `cargo test --lib env_path:: -- --nocapture`
Expected: existing `env_path` tests still PASS; the two new `#[cfg(windows)]` tests don't even compile into this run (we're on macOS) — confirm no `windows_shell`/`find_on_path` symbol errors appear, since they don't exist yet and the tests are cfg'd out on this platform. This step is a no-op safety check, not a real red-bar — the meaningful failure (missing symbols) would only show on a Windows target. Proceed to implementation.

- [ ] **Step 3: Implement `find_on_path` and `windows_shell`, wire into `login_shell`**

Replace the existing `login_shell` function (`src/env_path.rs:109-117`):

```rust
/// The user's login shell from `$SHELL`, accepted only when it's an absolute
/// path to an existing file. Anything else (relative path, stale entry, a
/// value injected by a hostile parent environment) falls back to `/bin/sh`.
///
/// On Windows there is no `$SHELL`/login-shell concept; see [`windows_shell`].
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

/// Windows shell choice: prefer `pwsh.exe` (PowerShell 7+, supports `&&`/`||`
/// chaining in lifecycle scripts) when it's on `PATH`, falling back to the
/// always-present `powershell.exe` (5.1, no `&&`/`||`) otherwise.
#[cfg(windows)]
fn windows_shell() -> String {
    if find_on_path("pwsh.exe").is_some() {
        "pwsh.exe".into()
    } else {
        "powershell.exe".into()
    }
}

/// True if `name` exists as a file in some directory on `$PATH`. Used only
/// for the pwsh/powershell probe above — this is a plain existence check, not
/// the PATHEXT-aware search agent binaries need (see `agent::resolve_on_path`).
#[cfg(windows)]
fn find_on_path(name: &str) -> Option<std::path::PathBuf> {
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .map(|dir| dir.join(name))
        .find(|p| p.is_file())
}
```

- [ ] **Step 4: No-op `ensure_login_path` on Windows**

Modify `needs_resolution` (`src/env_path.rs:65-67`):

```rust
fn needs_resolution() -> bool {
    // Windows GUI launches inherit the full user PATH from the registry —
    // there is no Finder/.desktop-style "thin PATH" problem to solve here.
    if cfg!(windows) {
        return false;
    }
    launched_without_terminal() || looks_thin()
}
```

- [ ] **Step 5: Run tests**

Run: `cargo test --lib env_path::`
Expected: all tests PASS (existing behavior unchanged on macOS; new Windows tests are cfg'd out here and will run on the Windows CI leg / a Windows machine in Task 5's verification).

- [ ] **Step 6: Commit**

```bash
git add src/env_path.rs
git commit -m "feat(windows): resolve pwsh/powershell as the Windows login shell"
```

---

## Task 2: Platform-conditional lifecycle-script invocation in `session.rs`

**Files:**
- Modify: `src/session.rs:163-187` (`spawn_script`)
- Test: `src/session.rs` (inline `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `crate::env_path::login_shell()` (Task 1, unchanged signature).
- Produces: no new public interface — `spawn_script` behavior changes only in which flag precedes the script argument.

- [ ] **Step 1: Read current implementation for context**

`src/session.rs:163-187` currently does:

```rust
    pub fn spawn_script(
        label: String,
        project: String,
        wt_path: String,
        script: &str,
        cwd: &str,
    ) -> Result<Self> {
        let mut cmd = CommandBuilder::new(crate::env_path::login_shell());
        cmd.arg("-lc");
        cmd.arg(script);
        cmd.cwd(cwd);
        cmd.env("TERM", "xterm-256color");
        cmd.env("LC_ALL", "en_US.UTF-8");

        Self::launch_pty(
            label,
            project,
            wt_path,
            Agent::Terminal,
            SessionBackend::Native,
            cmd,
            INIT_ROWS,
            INIT_COLS,
        )
    }
```

`-lc` is POSIX-shell syntax (`sh -lc "script"`) and has no meaning to `pwsh.exe`/`powershell.exe`, which expect `-Command "script"`.

- [ ] **Step 2: Write the failing test**

Add near the existing tests in `src/session.rs`'s `#[cfg(test)] mod tests` block (`src/session.rs:676`). This test only runs on Unix (spawns `true`, a POSIX shell builtin/binary); a Windows-side equivalent is exercised manually in Task 5 since there's no Windows CI test job. Grove has no existing test infra to intercept `CommandBuilder` args before spawn, so we test behaviorally: the same pattern the existing test at `src/app.rs:2241` already uses (spawn a real trivial script and check it runs).

```rust
    #[cfg(unix)]
    #[test]
    fn spawn_script_uses_posix_dash_l_c_flag_on_unix() {
        // Behavioral proxy for the flag choice: on Unix, a script containing
        // shell syntax only `-lc` (not `-Command`) can interpret should exit
        // successfully.
        let mut s = Session::spawn_script(
            "t".into(),
            "p".into(),
            ".".into(),
            "test -n \"$SHELL\" || true",
            ".",
        )
        .expect("spawn_script should succeed");
        std::thread::sleep(std::time::Duration::from_millis(200));
        s.kill();
    }
```

- [ ] **Step 3: Run test to verify it currently passes (baseline)**

Run: `cargo test --lib session::tests::spawn_script_uses_posix_dash_l_c_flag_on_unix -- --nocapture`
Expected: PASS (this confirms today's Unix behavior before the refactor — it's a regression guard, not a red/green TDD step, since the Unix path isn't changing).

- [ ] **Step 4: Make the flag platform-conditional**

Replace the two `cmd.arg(...)` lines in `spawn_script` (`src/session.rs:170-172`):

```rust
        let mut cmd = CommandBuilder::new(crate::env_path::login_shell());
        #[cfg(windows)]
        cmd.arg("-Command");
        #[cfg(not(windows))]
        cmd.arg("-lc");
        cmd.arg(script);
```

- [ ] **Step 5: Run tests**

Run: `cargo test --lib session::`
Expected: all PASS, including the new test from Step 2.

- [ ] **Step 6: Commit**

```bash
git add src/session.rs
git commit -m "feat(windows): invoke lifecycle scripts with -Command on Windows"
```

---

## Task 3: PATHEXT-aware agent resolution in `agent.rs`

**Files:**
- Modify: `src/agent.rs`
- Test: `src/agent.rs` (inline `#[cfg(test)] mod tests` — new module, none exists today)

**Interfaces:**
- Consumes: nothing new.
- Produces:
  - `Agent::invocation(self) -> (String, Vec<String>)` — new public method. Returns `(program, prefix_args)`; callers append their own args after `prefix_args`.
  - `resolve_on_path(name: &str) -> Option<PathBuf>` — new `#[cfg(windows)]` private function.
  - `Agent::available(self) -> bool` and `Agent::version(self) -> Option<String>` keep their existing signatures but are rewired internally to use `invocation()`/`resolve_on_path`.
  - Task 4 (`src/session.rs::spawn_native`) consumes `Agent::invocation()`.

- [ ] **Step 1: Write the failing tests**

`src/agent.rs` has no `#[cfg(test)]` module yet. Add one at the end of the file:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn resolve_on_path_finds_cmd_shim() {
        let dir = std::env::temp_dir().join("grove_test_agent_cmd_shim");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("claude.cmd"), b"").unwrap();

        let old_path = std::env::var_os("PATH");
        std::env::set_var("PATH", &dir);
        std::env::set_var("PATHEXT", ".COM;.EXE;.BAT;.CMD");

        let resolved = resolve_on_path("claude").expect("should find claude.cmd");
        assert_eq!(resolved.file_name().unwrap(), "claude.cmd");

        if let Some(p) = old_path {
            std::env::set_var("PATH", p);
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(windows)]
    #[test]
    fn resolve_on_path_prefers_exe_order_in_pathext() {
        let dir = std::env::temp_dir().join("grove_test_agent_exe_priority");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("codex.cmd"), b"").unwrap();
        std::fs::write(dir.join("codex.exe"), b"").unwrap();

        let old_path = std::env::var_os("PATH");
        std::env::set_var("PATH", &dir);
        // .EXE listed before .CMD: resolve_on_path must return codex.exe.
        std::env::set_var("PATHEXT", ".COM;.EXE;.BAT;.CMD");

        let resolved = resolve_on_path("codex").expect("should find a match");
        assert_eq!(resolved.file_name().unwrap(), "codex.exe");

        if let Some(p) = old_path {
            std::env::set_var("PATH", p);
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(windows)]
    #[test]
    fn invocation_wraps_cmd_shim_with_cmd_exe() {
        let dir = std::env::temp_dir().join("grove_test_agent_invocation");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("opencode.cmd"), b"").unwrap();

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
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn invocation_is_plain_binary_name_on_non_windows_or_when_unresolved() {
        // On non-Windows this is the only branch. On Windows, when nothing on
        // PATH matches, invocation() falls back to the bare name so the
        // existing "not found" UX (Agent::available() == false) is preserved.
        let (program, prefix_args) = Agent::Claude.invocation();
        assert!(prefix_args.is_empty() || cfg!(windows));
        #[cfg(not(windows))]
        assert_eq!(program, "claude");
    }
}
```

- [ ] **Step 2: Run tests to verify current state**

Run: `cargo test --lib agent::`
Expected: compile error — `resolve_on_path` and `Agent::invocation` don't exist yet. This is the real red bar (unlike Tasks 1-2, this module has no pre-existing test scaffold, so the compile failure itself is the "test fails" signal).

- [ ] **Step 3: Implement `resolve_on_path` and `invocation`**

Replace the `program`/`binary_name`/`available`/`version` region of `src/agent.rs` (lines 39-116) with:

```rust
    pub fn program(self) -> String {
        match self {
            Agent::Claude | Agent::Codex | Agent::OpenCode => self.binary_name().into(),
            // The user's login shell (validated), falling back to a POSIX shell.
            Agent::Terminal => crate::env_path::login_shell(),
        }
    }

    /// Executable name to look up on `$PATH`. `Terminal` has no static name
    /// (resolved at runtime via `$SHELL`) so callers must guard against it.
    fn binary_name(self) -> &'static str {
        match self {
            Agent::Claude => "claude",
            Agent::Codex => "codex",
            Agent::OpenCode => "opencode",
            Agent::Terminal => "",
        }
    }

    pub fn launch_args(self, skip_permissions: bool) -> Vec<String> {
        match self {
            Agent::Claude if skip_permissions => vec!["--dangerously-skip-permissions".into()],
            Agent::Codex if skip_permissions => {
                vec!["--dangerously-bypass-approvals-and-sandbox".into()]
            }
            _ => vec![],
        }
    }

    /// How to actually invoke this agent's CLI, as a `(program, prefix_args)`
    /// pair. Callers append their own args after `prefix_args`.
    ///
    /// On Unix (and `Terminal` everywhere) this is just `(binary_name, [])` —
    /// the OS execs it directly. On Windows, npm-installed CLIs like `claude`
    /// typically install as a `claude.cmd` shim, which `CreateProcess` can't
    /// execute directly (it isn't a PE binary); when `resolve_on_path` finds a
    /// `.cmd`/`.bat` match, this wraps it as `cmd.exe /C <resolved-path>`.
    /// `.exe` matches are run directly, matching Unix behavior.
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
                            return ("cmd.exe".into(), vec!["/C".into(), path.display().to_string()]);
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

    /// Returns true if the binary for this agent can be found on `$PATH` and
    /// has at least one execute bit set. `Terminal` is always available (it
    /// resolves via `$SHELL`). Returns `false` — never panics — when `$PATH`
    /// is unset.
    ///
    /// # Platform
    /// Unix checks the execute bit via `std::os::unix::fs::PermissionsExt`.
    /// Windows uses `resolve_on_path`, which applies a `%PATHEXT%`-aware
    /// search since Windows has no execute-bit concept and CLIs there are
    /// often extensionless-looking `.cmd` shims.
    pub fn available(self) -> bool {
        match self {
            Agent::Terminal => true,
            Agent::Claude | Agent::Codex | Agent::OpenCode => {
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
        }
    }

    /// Runs `<program> --version` and returns the trimmed first non-empty line
    /// of stdout — robust across the three CLIs' differing formats. Returns
    /// `None` if the agent has no static binary (`Terminal`), the command fails
    /// to spawn or run, or it yields no usable output; callers then fall back to
    /// displaying "installed". This shells out, so callers should run it off the
    /// UI thread.
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

/// Returns true if `path` is a regular file with at least one execute bit set.
/// Falls back to `is_file()` on non-Unix targets.
#[cfg(not(windows))]
fn is_executable(path: std::path::PathBuf) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(&path)
            .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

/// Windows-only: search every `$PATH` directory for `<name><ext>` across each
/// extension in `%PATHEXT%` (falling back to the standard `.COM;.EXE;.BAT;.CMD`
/// list if `PATHEXT` is unset), in `PATHEXT` order. Returns the first match,
/// mirroring how `cmd.exe`/Explorer resolve a bare command name.
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
```

Note: `is_executable` is now `#[cfg(not(windows))]` since Windows resolution goes entirely through `resolve_on_path` instead.

- [ ] **Step 4: Run tests**

Run: `cargo test --lib agent::`
Expected: PASS. On this (macOS) machine, only `invocation_is_plain_binary_name_on_non_windows_or_when_unresolved` and the existing behavior actually execute; the three `#[cfg(windows)]` tests are compiled out here and will run on the Windows CI leg / a Windows machine (Task 5's manual verification).

Also run the full suite to confirm nothing else broke:

Run: `cargo test`
Expected: all PASS.

- [ ] **Step 5: Commit**

```bash
git add src/agent.rs
git commit -m "feat(windows): PATHEXT-aware agent binary resolution and invocation"
```

---

## Task 4: Wire `Agent::invocation()` into `spawn_native`

**Files:**
- Modify: `src/session.rs:127-155` (`spawn_native`)

**Interfaces:**
- Consumes: `Agent::invocation(self) -> (String, Vec<String>)` (Task 3).
- Produces: no new interface — `spawn_native`'s external behavior (signature, callers) is unchanged; only how it builds `CommandBuilder` changes.

- [ ] **Step 1: Write a regression test**

Add to `src/session.rs`'s `#[cfg(test)] mod tests` block, alongside the Task 2 test:

```rust
    #[test]
    fn spawn_native_still_launches_terminal_agent() {
        // Regression guard for the invocation() rewiring: Agent::Terminal must
        // keep working exactly as before (it bypasses invocation()'s
        // Windows-shim branch entirely).
        let mut s = Session::spawn(
            "t".into(),
            "p".into(),
            ".".into(),
            Agent::Terminal,
            &[],
            ".",
            false,
        )
        .expect("spawn_native via Agent::Terminal should succeed");
        std::thread::sleep(std::time::Duration::from_millis(200));
        s.kill();
    }
```

- [ ] **Step 2: Run test to verify it currently passes (baseline)**

Run: `cargo test --lib session::tests::spawn_native_still_launches_terminal_agent -- --nocapture`
Expected: PASS (baseline, before the refactor below).

- [ ] **Step 3: Rewire `spawn_native` to use `invocation()`**

Replace the start of `spawn_native` (`src/session.rs:127-138`):

```rust
    fn spawn_native(
        label: String,
        project: String,
        wt_path: String,
        agent: Agent,
        args: &[String],
        cwd: &str,
    ) -> Result<Self> {
        let (program, prefix_args) = agent.invocation();
        let mut cmd = CommandBuilder::new(program);
        for a in prefix_args {
            cmd.arg(a);
        }
        for a in args {
            cmd.arg(a);
        }
        cmd.cwd(cwd);
        cmd.env("TERM", "xterm-256color");
        // Ensure the agent emits UTF-8 even when grove is launched from a macOS
        // .app bundle (which inherits no UTF-8 locale from the shell).
        cmd.env("LC_ALL", "en_US.UTF-8");
```

(the rest of the function — the `Self::launch_pty(...)` call — is unchanged)

- [ ] **Step 4: Run tests**

Run: `cargo test --lib session::`
Expected: all PASS, including both the Step 1 test and the Task 2 test.

Run: `cargo test`
Expected: all PASS.

- [ ] **Step 5: Commit**

```bash
git add src/session.rs
git commit -m "feat(windows): route spawn_native through Agent::invocation"
```

---

## Task 5: Windows CI matrix leg + `.msi` packaging

**Files:**
- Create: `wix/main.wxs`
- Modify: `.github/workflows/release.yml`

**Interfaces:**
- Consumes: `Cargo.toml`'s `[package]` `name`/`version` (existing, unchanged) for the WiX `Product` `Name`/`Version` fields.
- Produces: a `Grove-<version>-windows-x86_64-alpha.msi` release artifact.

- [ ] **Step 1: Add the WiX template**

Create `wix/main.wxs`:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<Wix xmlns="http://schemas.microsoft.com/wix/2006/wi">
  <Product
    Id="*"
    Name="Grove (Alpha)"
    UpgradeCode="8f3b2e0a-6c1d-4a7a-9e3c-2b6a1d4f9c11"
    Language="1033"
    Codepage="UTF-8"
    Version="$(var.Version)"
    Manufacturer="Grove">

    <Package
      Id="*"
      Keywords="Installer"
      Description="Grove (Alpha) — a worktree launchpad for AI coding agents"
      Manufacturer="Grove"
      InstallerVersion="500"
      Languages="1033"
      Compressed="yes"
      InstallScope="perUser"
      SummaryCodepage="1252" />

    <MajorUpgrade
      Schedule="afterInstallInitialize"
      DowngradeErrorMessage="A newer version of Grove is already installed." />

    <Media Id="1" Cabinet="grove.cab" EmbedCab="yes" />

    <Directory Id="TARGETDIR" Name="SourceDir">
      <Directory Id="LocalAppDataFolder">
        <Directory Id="APPLICATIONFOLDER" Name="Grove" />
      </Directory>
      <Directory Id="ProgramMenuFolder">
        <Directory Id="ApplicationProgramsFolder" Name="Grove" />
      </Directory>
    </Directory>

    <DirectoryRef Id="APPLICATIONFOLDER">
      <Component Id="GroveExecutable" Guid="*">
        <File
          Id="GroveEXE"
          Source="target\release\grove.exe"
          KeyPath="yes" />
      </Component>
    </DirectoryRef>

    <DirectoryRef Id="ApplicationProgramsFolder">
      <Component Id="ApplicationShortcut" Guid="*">
        <Shortcut
          Id="ApplicationStartMenuShortcut"
          Name="Grove (Alpha)"
          Description="A worktree launchpad for AI coding agents"
          Target="[APPLICATIONFOLDER]grove.exe"
          WorkingDirectory="APPLICATIONFOLDER" />
        <RemoveFolder Id="ApplicationProgramsFolder" On="uninstall" />
        <RegistryValue
          Root="HKCU"
          Key="Software\Grove"
          Name="installed"
          Type="integer"
          Value="1"
          KeyPath="yes" />
      </Component>
    </DirectoryRef>

    <Feature Id="MainApplication" Title="Grove" Level="1">
      <ComponentRef Id="GroveExecutable" />
      <ComponentRef Id="ApplicationShortcut" />
    </Feature>
  </Product>
</Wix>
```

This installs per-user (no admin elevation needed), adds a Start Menu shortcut, and registers a standard uninstall entry via `MajorUpgrade`/`Feature`. No custom icon is wired in for v1 — `assets/icon/*.png` aren't in `.ico` format and there's no conversion tooling in this repo or guaranteed on the `windows-latest` runner; the shortcut uses Windows' default executable icon. Revisit if/when an `.ico` asset is added.

- [ ] **Step 2: Add the Windows matrix entry**

Modify the `matrix.include` list in `.github/workflows/release.yml` (after line 31, the existing `linux`/`amd64` entry):

```yaml
          - os: linux
            arch: amd64
            runner: ubuntu-latest
          - os: windows
            arch: x86_64
            runner: windows-latest
```

- [ ] **Step 3: Branch the build/package steps for Windows**

The existing steps (`Install Linux build deps`, `Install cargo-bundle`, `Build release bundle`, `Package artifacts`) are Unix-oriented (`shell: bash` on the packaging step, `cargo bundle`). Add Windows-specific steps and guard the existing ones so they skip on Windows. Modify `.github/workflows/release.yml` from the `Install Linux build deps` step (line 48) through the end of `Package artifacts` (line 91):

```yaml
      - name: Install Linux build deps
        if: matrix.os == 'linux'
        run: |
          sudo apt-get update
          # GUI (iced) + bundling prerequisites.
          sudo apt-get install -y \
            libgtk-3-dev libxdo-dev libayatana-appindicator3-dev \
            librsvg2-dev libssl-dev pkg-config dpkg

      - name: Install cargo-bundle
        if: matrix.os != 'windows'
        run: cargo install cargo-bundle --locked || cargo install cargo-bundle

      - name: Install cargo-wix
        if: matrix.os == 'windows'
        run: cargo install cargo-wix --locked || cargo install cargo-wix

      - name: Build release bundle (macOS/Linux)
        if: matrix.os != 'windows'
        run: cargo bundle --release

      - name: Build release binary (Windows)
        if: matrix.os == 'windows'
        run: cargo build --release

      - name: Package artifacts (macOS/Linux)
        if: matrix.os != 'windows'
        id: package
        shell: bash
        run: |
          set -euo pipefail
          VERSION="$(grep -m1 '^version' Cargo.toml | sed -E 's/.*"(.*)".*/\1/')"
          OUT="dist"
          mkdir -p "$OUT"

          if [ "${{ matrix.os }}" = "macos" ]; then
            APP="$(find target/release/bundle/osx -maxdepth 1 -name '*.app' | head -n1)"
            [ -n "$APP" ] || { echo "no .app produced" >&2; exit 1; }
            # Ad-hoc sign: an unsigned .app downloaded via browser gets the
            # com.apple.quarantine xattr, and Gatekeeper reports totally
            # unsigned + quarantined apps as "damaged" rather than the usual
            # "unidentified developer" prompt. Ad-hoc signing (no cert/Apple
            # account needed) gives it a valid signature so Gatekeeper falls
            # back to the normal unidentified-developer flow.
            codesign --force --deep -s - "$APP"
            # Wrap the .app in a .dmg for distribution.
            DMG="$OUT/Grove-${VERSION}-macos-${{ matrix.arch }}.dmg"
            hdiutil create -volname "Grove" -srcfolder "$APP" -ov -format UDZO "$DMG"
            echo "Built $DMG"
          else
            DEB="$(find target/release/bundle/deb -maxdepth 1 -name '*.deb' | head -n1)"
            [ -n "$DEB" ] || { echo "no .deb produced" >&2; exit 1; }
            cp "$DEB" "$OUT/Grove-${VERSION}-linux-amd64.deb"
            echo "Built $OUT/Grove-${VERSION}-linux-amd64.deb"
          fi

      - name: Package artifacts (Windows)
        if: matrix.os == 'windows'
        shell: bash
        run: |
          set -euo pipefail
          VERSION="$(grep -m1 '^version' Cargo.toml | sed -E 's/.*"(.*)".*/\1/')"
          mkdir -p dist
          cargo wix --no-build --nocapture -o "dist/Grove-${VERSION}-windows-x86_64-alpha.msi"
          echo "Built dist/Grove-${VERSION}-windows-x86_64-alpha.msi"
```

`cargo wix` auto-discovers `wix/main.wxs` and reads the package version from `Cargo.toml` by default, so no extra `-D Version=...` wiring is needed. `--no-build` reuses the binary already produced by the "Build release binary (Windows)" step instead of rebuilding.

- [ ] **Step 4: Verify the workflow YAML is well-formed**

Run: `cd /Users/digvijaymahapatra/Sandbox/grove && python3 -c "import yaml, sys; yaml.safe_load(open('.github/workflows/release.yml'))" && echo "YAML OK"`
Expected: `YAML OK`. (If `pyyaml` isn't installed, use `ruby -ryaml -e "YAML.load_file('.github/workflows/release.yml'); puts 'YAML OK'"` instead — either confirms the file parses.)

- [ ] **Step 5: Commit**

```bash
git add wix/main.wxs .github/workflows/release.yml
git commit -m "feat(windows): add Windows CI matrix leg with cargo-wix MSI packaging"
```

- [ ] **Step 6: Note on verification limits**

This task's correctness (does `cargo build --release` actually succeed on Windows, does `cargo wix` actually produce an installable `.msi`, does the installed app actually launch and run sessions) can only be confirmed by:
1. The `windows-latest` CI runner, the next time a release is drafted (per the workflow's `on: release: types: [...]` trigger) — this is the real integration test for Tasks 1-5 together.
2. Manual testing on an actual Windows machine, per the spec's "Testing" section (build, install the `.msi`, launch from Start Menu, run each agent, verify PTY rendering/resize/copy-paste/kill).

There is no Windows GUI test runner available in this development environment (macOS) or in Grove's CI today (no `cargo test` step exists in `release.yml`, and adding one is out of scope for this plan). Flag to the user that a real Windows smoke test is the recommended next step after this plan lands, ideally by drafting a prerelease and installing the resulting `.msi`.

---

## Task 6: Update README for Windows (alpha)

**Files:**
- Modify: `README.md`

**Interfaces:**
- Consumes: nothing.
- Produces: nothing consumed by other tasks — purely user-facing documentation.

- [ ] **Step 1: Update the platform badge**

Replace `README.md:10`:

```markdown
[![platform: linux | macOS | Windows (alpha)](https://img.shields.io/badge/platform-linux%20%7C%20macOS%20%7C%20Windows%20(alpha)-7aa2f7?style=flat-square)](#requirements)
```

- [ ] **Step 2: Add a Windows install note**

After the existing platform bullets in the `## install` section (`README.md:59-64`), add a Windows bullet and note:

```markdown
`install.sh` builds a release bundle with [`cargo-bundle`] (installing it on first run) and installs grove as a clickable native app:

- **macOS** — copies `Grove.app` to `/Applications` (or `~/Applications`). launch it from Spotlight or Launchpad.
- **linux** — installs the generated `.deb` via `dpkg`, or falls back to a binary plus a `grove.desktop` launcher and icon under `~/.local`. launch "Grove" from your application menu.
- **windows (alpha)** — no `install.sh` support yet; download the `.msi` from the [latest release](https://github.com/gitfudge0/grove/releases/latest) and run it. windows support is new and less battle-tested than macOS/linux — expect rough edges, and please file issues.

when launched from a desktop menu or app launcher, grove recovers your login `PATH` from your shell on startup, so it can still find `claude`, `git`, and your agents. set `GROVE_FORCE_LOGIN_PATH=1` to force this even from a terminal. on windows, grove uses `pwsh` (PowerShell 7+) when available, falling back to the built-in `powershell.exe`.
```

- [ ] **Step 3: Update the requirements list**

Replace `README.md:141-146`:

```markdown
## requirements

- rust toolchain (`cargo`) for installation from source
- `git`
- linux, macOS, or windows (alpha) with a graphical desktop session
- `tmux` (optional, recommended for persistent sessions — macOS/linux only; windows always runs native sessions)
```

- [ ] **Step 4: Proofread rendered Markdown**

Run: `grep -n "windows" README.md`
Expected: three new mentions appear (badge, install bullet/note, requirements line), each readable in context — no dangling sentence fragments or broken Markdown links.

- [ ] **Step 5: Commit**

```bash
git add README.md
git commit -m "docs(windows): document alpha Windows support"
```

---

## Final Verification

- [ ] Run the full test suite one more time: `cargo test` — expected: all PASS.
- [ ] Run `cargo clippy --all-targets -- -D warnings` — expected: no warnings (Grove's existing lint bar; the new `#[cfg(windows)]` code should not introduce cross-platform-only warnings visible on macOS, but if clippy flags anything on this platform, fix before considering the plan done).
- [ ] Confirm `git log --oneline -7` shows the six commits from Tasks 1-6 in order, each independently reviewable.
- [ ] Report to the user: implementation is complete and verified on macOS/Linux code paths; a real Windows build/run verification (drafting a prerelease, installing the `.msi`, smoke-testing sessions) is the recommended immediate next step, since no Windows runner exists in this development environment.
