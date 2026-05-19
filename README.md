# Grove

A terminal worktree launchpad for AI coding agents. Grove manages git worktrees across multiple projects and launches AI coding agents (Claude Code, Codex, OpenCode) directly into them.

## Install

One-liner (requires `git`, `cargo`/Rust):

```sh
curl -fsSL https://raw.githubusercontent.com/gitfudge0/grove/main/install.sh | bash
```

Or clone and install locally:

```sh
git clone https://github.com/gitfudge0/grove.git
cd grove
./install.sh
```

This installs the `grove` binary to `$CARGO_HOME/bin` (default `~/.cargo/bin`). Make sure that directory is on your `PATH`.

To uninstall:

```sh
./uninstall.sh
```

## Usage

Run `grove` in your terminal. The TUI shows two panes:

- **Projects** — git repositories you've registered with Grove.
- **Worktrees** — worktrees for the selected project.

Add a project, create a worktree, and press the launch key to drop into an agent session inside that worktree.

### Supported agents

- `claude` (Claude Code)
- `codex`
- `opencode`

Each agent must be installed and available on your `PATH`.

## Requirements

- Rust toolchain (`cargo`)
- `git`
- A Unix-like terminal (macOS / Linux)

## License

MIT
