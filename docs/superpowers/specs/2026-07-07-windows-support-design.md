# Windows support (alpha)

**Date:** 2026-07-07
**Status:** Approved design, pending implementation plan

## Problem

Grove currently only builds and ships for macOS (arm64/x86_64) and Linux
(amd64). The core dependencies (`portable-pty`, `arboard`, `keepawake`, `rfd`)
all have native Windows backends, and most platform-specific code already
degrades gracefully off-macOS, but three things stand in the way of a working
Windows build:

1. Session persistence relies on shelling out to `tmux`, which doesn't exist
   on Windows.
2. Login-shell / PATH resolution and lifecycle-script invocation
   (`src/env_path.rs`) assume a POSIX shell.
3. Agent CLI discovery and invocation (`src/agent.rs`) assumes an
   extensionless executable directly runnable by `CreateProcess`, which
   breaks for npm-installed CLIs on Windows (`claude.cmd`, `codex.cmd`, etc).

This is a first release: **Windows support ships as an alpha**. It should be
clearly labeled as such wherever a user would notice it (release artifact
name, release notes), so early adopters know to expect rough edges
(nothing about the core session/rendering model becomes conditionally
"more stable" later — "alpha" here means "less battle-tested," not
"feature-incomplete by design").

## Goals

- `cargo build --release` produces a working `grove.exe` on Windows.
- CI produces an installable `.msi` attached to GitHub Releases alongside the
  existing macOS/Linux bundles.
- Agent sessions (Claude/Codex/OpenCode/Terminal) launch and run correctly via
  ConPTY, using PowerShell as the interactive/lifecycle shell.
- Existing keyboard shortcuts, clipboard, sleep-prevention, and file dialogs
  work unchanged (all already have Windows-compatible crate backends).

## Non-goals

- tmux-equivalent session persistence across Grove restarts on Windows.
  Native (non-tmux) sessions already exist as a fallback path for
  tmux-less environments; Windows always uses that path.
- Killing a native session's full process tree on Windows (e.g. via Job
  Objects). v1 keeps the existing non-Unix fallback, which kills only the
  direct child process. Documented as a known limitation.
- Code signing / notarization equivalent for the `.msi`. It ships unsigned,
  matching the "no cert" posture of the ad-hoc-signed macOS build. Expect
  SmartScreen warnings on first run.
- WSL integration of any kind.

## Design

### 1. Session backend — no tmux on Windows

`Session::spawn` already branches on a `use_tmux: bool` into `spawn_tmux` /
`spawn_native`, and `tmux::available()` already returns `false` when `tmux`
isn't on `PATH`. No code change is required here: on Windows, `tmux` is
realistically never present, so every session takes the existing native path.

`kill_native`'s process-tree kill (`libc::killpg`) is already `#[cfg(unix)]`
with a working fallback (`c.kill()`, direct child only) for other platforms.
Windows uses that fallback unchanged in v1.

### 2. Shell resolution (`src/env_path.rs`)

- `login_shell()` gains a Windows branch: probe `PATH` for `pwsh.exe` first
  (PowerShell 7+, supports `&&`/`||` chaining), falling back to the always-
  present `powershell.exe` (5.1, no `&&`/`||`) if `pwsh` isn't found. Unix
  behavior (`$SHELL` validation, `/bin/sh` fallback) is untouched.
- `spawn_script`'s invocation becomes platform-conditional: Unix keeps
  `<shell> -lc <script>`; Windows uses `<shell> -Command <script>`.
- `ensure_login_path`'s "thin PATH from a GUI launch" heuristic
  (`needs_resolution`, `looks_thin`, `query_login_path`) is a Unix-specific
  problem (Finder/.desktop launches inheriting a stripped environment).
  Windows GUI launches inherit the full user `PATH` from the registry, so
  `ensure_login_path` becomes a no-op on Windows (`needs_resolution` returns
  `false` immediately on that target) rather than growing a parallel
  Windows-specific probing path.

### 3. Agent binary resolution (`src/agent.rs`)

This is the trickiest part: `claude`, `codex`, and `opencode` are npm-
installed CLIs that on Windows install as `<name>.cmd` shims, not directly
executable PE binaries.

- `Agent::available()`'s search (`dir.join(name)` + `is_executable`) assumes
  an extensionless exact match. On Windows it becomes PATHEXT-aware: for each
  `PATH` directory, try `<name>` plus each extension in `%PATHEXT%` (falling
  back to a `.exe`/`.cmd`/`.bat` default list if `PATHEXT` is unset), and
  treat any hit as available (`is_file()` — Windows has no execute-bit
  concept, matching the existing non-Unix fallback in `is_executable`).
- Invocation changes to match: `spawn_native` (session.rs) and
  `Agent::version()` (agent.rs) can't `Command::new("claude")` directly when
  the resolved binary is a `.cmd`/`.bat` shim (`CreateProcess` can't execute
  script files). On Windows, when the resolved match is `.cmd`/`.bat`, spawn
  it via `cmd.exe /C <resolved-path> <args>`; when it's `.exe`, spawn
  directly as today. `.ps1` is out of scope (none of the three CLIs ship
  one).

### 4. CI / packaging (`.github/workflows/release.yml`)

- Add a `windows-latest` / `amd64` matrix entry.
- `cargo-bundle` doesn't support Windows, so the build step branches: on
  Windows, run `cargo build --release` directly (no `cargo bundle`).
- Package with `cargo-wix` into an unsigned `.msi`:
  - New `wix/main.wxs` template: per-user install, Start Menu shortcut,
    standard uninstall entry, using the existing `assets/icon` set.
  - Product/display name in the installer reads **"Grove (Alpha)"** so the
    alpha status is visible at install time, independent of release notes.
- Output artifact name: `Grove-<version>-windows-x86_64-alpha.msi`, uploaded
  to the release alongside the macOS/Linux artifacts.
- Release notes / announcement copy for the first Windows-inclusive release
  should call out alpha status and known limitations (no session persistence
  across restarts, partial process-tree cleanup on kill).

### 5. Unaffected by design (verified, no change needed)

- **Keyboard shortcuts** (`gui/update.rs`): `global_mods`,
  `new_session_in_worktree_mods`, and `platform_mod_label` already branch on
  `not(target_os = "macos")` generically — Windows gets the same
  Ctrl-based bindings and label as Linux for free.
- **Dock integration** (`gui/dock.rs`): already `#[cfg(not(target_os =
  "macos"))]` no-op stubs.
- **Clipboard** (`arboard`), **sleep prevention** (`keepawake`), **file
  dialogs** (`rfd`), **PTY** (`portable-pty`): all have native Windows
  backends already pulled in by the existing dependency versions; no
  `Cargo.toml` changes expected.

## Affected files

- `.github/workflows/release.yml` — Windows matrix entry, `.msi` packaging.
- `wix/main.wxs` — new WiX installer template.
- `src/env_path.rs` — Windows shell resolution (`pwsh`/`powershell` probe),
  Windows no-op for `ensure_login_path`, platform-conditional script
  invocation syntax.
- `src/agent.rs` — PATHEXT-aware `available()`/binary resolution,
  `.cmd`/`.bat` invocation via `cmd.exe /C`.
- `src/session.rs` — use the resolved Windows invocation from `agent.rs` in
  `spawn_native`/`spawn_script`; no behavioral change to the tmux/native
  branch itself.

## Testing

- Unit: `login_shell()` Windows branch (pwsh found / not found), PATHEXT
  resolution in `agent.rs` against a fake `PATH` with `.cmd`/`.exe` fixtures.
- Manual (on Windows, since CI has no interactive GUI test): build via
  `cargo build --release`, install the `.msi`, launch Grove from Start Menu,
  start a session with each agent (or Terminal if none installed), verify
  PTY rendering, resize, copy/paste, and kill-session all work.
- CI: the new matrix leg must produce a `.msi` artifact successfully; this is
  a build/package smoke test, not a runtime test.
