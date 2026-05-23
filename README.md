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

Add a project, create a worktree, and press enter to start an agent session inside that worktree. Sessions run in embedded PTYs — switch between them, the project/worktree browser, and the live agent at any time.

### Sessions

Agents run as managed sessions rather than replacing Grove. The session view has a **Sessions** sidebar on the left (every project → worktree → session) and the active session's PTY on the right. `Ctrl-g` toggles keyboard focus between the two: press it from the PTY to move into the sidebar (where you can browse sessions); press it again (or `enter`) to drop back into the PTY. Every other keystroke in the PTY is forwarded to the agent.

Grove supports two session backends:

- **tmux mode** — recommended when `tmux` is installed. Agent sessions run inside Grove-owned tmux sessions, persist after Grove exits, and are rediscovered on the next launch.
- **native mode** — no tmux dependency. Agent sessions run as direct embedded PTY children and end when Grove exits.

On first launch with `tmux` installed, Grove asks which backend to use. Press `m` later to open the tmux settings modal, toggle tmux for new sessions, and copy the optional tmux config snippet. Existing sessions keep the backend they were started with.

In the projects/worktrees Browser:

- `j` / `k` (or `↑` / `↓`) — move
- `h` / `l` (or `←` / `→`, or `tab`) — switch between projects and worktrees
- `enter` — open/focus a session for the worktree (uses the default agent)
- `c` / `C` — new session for the worktree (default / pick)
- `t` — new terminal in the worktree
- `T` — theme picker
- `a` / `d` — add / delete   `r` — refresh
- `m` — tmux settings and setup snippet
- `Ctrl-g` — jump to the active session pane
- `?` — help   `q` / `esc` — quit

In the Sessions sidebar:

- `j` / `k` — next / previous session (updates the right pane live)   `1`–`9` — jump to session N
- `Ctrl-g` / `enter` — focus the session's PTY
- `esc` — back to the projects browser
- `d` — kill the active session (worktree stays)
- `c` / `C` — new session for the active worktree (default / pick)
- `t` — new terminal for the active session's worktree

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
- `tmux` is recommended for persistent sessions, but not required
- A Unix-like terminal (macOS / Linux)

## License

MIT
