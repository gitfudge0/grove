# Grove

A terminal worktree launchpad for AI coding agents. Grove manages git worktrees across multiple projects and runs AI coding agents (Claude Code, Codex, OpenCode) in embedded terminal sessions inside each worktree, so you can keep several agents running side by side without ever leaving Grove.

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

Add a project, create a worktree, and press enter to start an agent session inside that worktree. Sessions run in embedded PTYs and stay alive in the background — switch between them, the project/worktree browser, and the live agent at any time.

### Sessions

Agents run as managed sessions rather than replacing Grove. From the browser, press `Ctrl-g` to jump to the active session. Inside a session pane, `Ctrl-g` is the leader key:

- `<leader>g` / `esc` — back to the browser
- `<leader>n` / `<leader>p` — next / previous session
- `<leader>1`–`9` — jump to session N
- `<leader>c` — new session with the default agent
- `<leader>C` — new session, pick an agent
- `<leader>t` — open a plain terminal for this worktree
- `<leader>x` — kill the current session
- `<leader><leader>` — send a literal `Ctrl-g` to the agent

Projects and worktrees with running sessions are marked with a green ● count in the browser.

### Supported agents

- `claude` (Claude Code)
- `codex`
- `opencode`
- `terminal` — your login shell, for ad-hoc work in a worktree

Each agent must be installed and available on your `PATH`.

## Requirements

- Rust toolchain (`cargo`)
- `git`
- A Unix-like terminal (macOS / Linux)

## License

MIT
