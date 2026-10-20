<div align="center">

# Grove

**Your projects. Your worktrees. Your agents. One native app.**

Run Codex, Claude Code, OpenCode, and terminals side by side, without losing your place.

[Download Grove](https://github.com/gitfudge0/grove/releases/latest) · [Release notes](RELEASE_NOTES.md) · [MIT](LICENSE-MIT) / [Apache-2.0](LICENSE-APACHE)

![Grove in dark mode, with three native Codex conversations across the Studio, Orbit, and Atlas project worktrees](assets/showcase/grove-grid.png)

<sub>Actual Grove UI. Fictional projects and conversations.</sub>

</div>

## Install

Download your installer from the [latest release](https://github.com/gitfudge0/grove/releases/latest):

- **macOS:** open the `.dmg` for Apple Silicon (`arm64`) or Intel (`x86_64`), then drag Grove to Applications.
- **Linux:** install the `.deb` for `amd64` with `sudo apt install ./Grove-*.deb`. Requires a graphical X11 or XWayland session.
- **Windows (alpha):** run the `.msi`. Expect rough edges.

If an installer is unavailable, build from source on macOS or Linux (requires Rust, Git, and the [development prerequisites](#develop)):

```sh
curl -fsSL https://raw.githubusercontent.com/gitfudge0/grove/main/install.sh | bash
```

Install [Git](https://git-scm.com/) and your preferred agent CLI separately, and sign in through that CLI. Grove uses the tools already on your machine. `tmux` is optional on macOS and Linux for persistent terminal sessions.

Open Grove, add a project folder, then start a conversation in the main checkout or create a worktree. Enable **Settings → Native Agents (experimental)** for native Codex and Claude Code transcripts on macOS and Linux. Toggle grid mode with **⌘G** on macOS or **Ctrl+Shift+G** on Linux.

### macOS security prompt

If macOS blocks Grove, try opening it once, then go to **System Settings → Privacy & Security → Open Anyway** ([Apple’s instructions](https://support.apple.com/102445)).

If needed, remove quarantine from the downloaded Grove app only, then reopen it:

```sh
xattr -dr com.apple.quarantine /Applications/Grove.app
```

## Develop

Install [Rust](https://rustup.rs/) and Git. On macOS, install the command line tools with `xcode-select --install`. Linux needs the system packages listed in the [build workflow](.github/workflows/release.yml); Windows uses Rust’s MSVC toolchain and the C++ build tools.

```sh
git clone https://github.com/gitfudge0/grove.git
cd grove
cargo run
```

To build and install a release app locally on macOS or Linux:

```sh
./install.sh
```

The script installs `cargo-bundle` if needed, builds Grove, and installs the native app. On Windows, use `cargo run` for development.

To disable anonymous usage telemetry, turn off **Share anonymous usage data** in Settings or set `GROVE_TELEMETRY=off`.
