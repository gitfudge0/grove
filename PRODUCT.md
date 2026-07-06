# Product

## Register

product

## Users

Developers who run multiple AI coding agents (Claude Code, Codex, OpenCode) across multiple repositories and worktrees, and don't want a tab graveyard or a forest of detached tmux sessions to keep track of them.

Context of use: a developer is working across several repositories, terminals, and editors, often with multiple agents running in parallel on different branches. They are switching between supervising one agent, kicking off another, and reading the latest output of a third. They want one persistent desktop surface that owns this orchestration without creating another pile of terminal windows.

Job to be done: register projects, create and destroy worktrees, spawn agent sessions in those worktrees, jump between live sessions, and never lose work when context-switching.

## Product Purpose

Grove is a worktree launchpad. It manages git worktrees across projects and runs AI coding agents in embedded PTY sessions inside each worktree, so several agents can run side by side without the developer leaving Grove.

Success looks like: the user opens Grove once at the start of the day, and it stays open. Every agent they spawn lives in a session that survives navigation. Switching from "the agent working on feature A" to "the agent working on feature B" is two keystrokes, not a window-manager dance.

## Brand Personality

Three words: **terminal-native, quiet, fast.**

Voice and tone: direct, lowercase-leaning, no marketing puff. Keybindings are first-class citizens, but the app does not hide behind them. Help text reads like a focused developer tool, not a SaaS onboarding tooltip. The product trusts the user to read concise labels and infer the rest.

Emotional goal: the calm of a well-tiled tmux setup, without the configuration. The user should feel like they're using a tool a senior engineer built for themselves.

## Anti-references

- **Electron "terminal" apps** with rounded cards, gradient hero metrics, and a sidebar full of avatars. Grove is a desktop app, but the PTY and worktree content stay visually primary.
- **VS Code chrome aesthetics** copied wholesale: heavy chrome, dense icon stacks, decorative emoji in menus. Grove should feel like a focused work surface, not an IDE clone.
- **Agent dashboards** that look like a Vercel project page. No status cards, no progress rings, no "AI is thinking…" shimmer.
- **Generic CLI wizards** that prompt-question-prompt-question. Grove is modal and keyboard-driven, not conversational.

## Design Principles

1. **Keyboard is a first-class interface.** Common actions are reachable without leaving the keyboard, while mouse controls stay compact and predictable.
2. **Sessions are the unit of work, not windows.** The UI exists to spawn, list, and jump between sessions. Everything else (projects, worktrees) is a path to a session.
3. **Show state, not chrome.** A green ● next to a project means "something is running there." That's the entire status system. No badges, no toast notifications, no progress bars.
4. **Theme is the user's, not ours.** Grove ships 40 themes because developers already have opinions about terminal colors. The default is a recognizable one (TokyoNight); everything else is the user's choice.
5. **One screen, three views.** Tree, activity, and terminal views cover the core workflows without nested navigation stacks.

## Accessibility & Inclusion

- Color is never the sole carrier of state. The session count `●N` pairs the dot with a numeral; focus is signaled by border emphasis plus title text, not color alone.
- Both light and dark theme families are first-class: 30 dark themes and 10 light themes. No "dark mode is correct" assumption.
- Reduced motion: motion is functional and restrained; nothing pulses, shimmers, or performs decorative animation.
- All common actions are keyboard-reachable. Embedded PTY accessibility remains bounded by the terminal applications Grove hosts.
