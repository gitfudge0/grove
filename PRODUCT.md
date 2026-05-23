# Product

## Register

product

## Users

Developers who run multiple AI coding agents (Claude Code, Codex, OpenCode) across multiple repositories and worktrees, and don't want a tab graveyard or a forest of detached tmux sessions to keep track of them.

Context of use: a developer is at their terminal, often with several agents working in parallel on different branches of different projects. They are switching between supervising one agent, kicking off another, and reading the latest output of a third. They want one persistent surface that owns this orchestration without making them leave the terminal.

Job to be done: register projects, create and destroy worktrees, spawn agent sessions in those worktrees, jump between live sessions, and never lose work when context-switching.

## Product Purpose

Grove is a worktree launchpad. It manages git worktrees across projects and runs AI coding agents in embedded PTY sessions inside each worktree, so several agents can run side by side without the developer leaving Grove.

Success looks like: the user opens Grove once at the start of the day, and it stays open. Every agent they spawn lives in a session that survives navigation. Switching from "the agent working on feature A" to "the agent working on feature B" is two keystrokes, not a window-manager dance.

## Brand Personality

Three words: **terminal-native, quiet, fast.**

Voice and tone: direct, lowercase-leaning, no marketing puff. Keybindings are first-class citizens. Help text reads like a vim plugin's docs, not a SaaS onboarding tooltip. The product trusts the user to read one line of footer hints and infer the rest.

Emotional goal: the calm of a well-tiled tmux setup, without the configuration. The user should feel like they're using a tool a senior engineer built for themselves.

## Anti-references

- **Electron "terminal" apps** with rounded cards, gradient hero metrics, and a sidebar full of avatars. Grove is a TUI; pretend the cursor never left the buffer.
- **VS Code chrome aesthetics** ported into the terminal: heavy borders, icon fonts, decorative emoji in menus. The terminal has eight semantic colors and a cursor — that's the toolkit.
- **Agent dashboards** that look like a Vercel project page. No status cards, no progress rings, no "AI is thinking…" shimmer.
- **Generic CLI wizards** that prompt-question-prompt-question. Grove is modal and keyboard-driven, not conversational.

## Design Principles

1. **Keyboard is the interface.** Every action has a single-key binding visible in the footer. The mouse is not a target. If something needs three keys, it earns them — `Ctrl-g` is load-bearing because session-PTY focus is load-bearing.
2. **Sessions are the unit of work, not windows.** The UI exists to spawn, list, and jump between sessions. Everything else (projects, worktrees) is a path to a session.
3. **Show state, not chrome.** A green ● next to a project means "something is running there." That's the entire status system. No badges, no toast notifications, no progress bars.
4. **Theme is the user's, not ours.** Grove ships 36 themes because developers already have opinions about terminal colors. The default is a recognizable one (TokyoNight); everything else is the user's choice.
5. **One screen, two modes.** Browser (projects/worktrees) and Session (sidebar + live PTY). Modals exist for input only. No tabs, no nested navigation stacks.

## Accessibility & Inclusion

- Color is never the sole carrier of state. The session count `●N` pairs the dot with a numeral; focus is signaled by border emphasis plus title text, not color alone.
- Both light and light-on-dark and dark-on-light theme families are first-class — 22 dark themes, 14 light. No "dark mode is correct" assumption.
- Reduced motion: there is effectively no motion. Terminal redraws are the only animation; nothing pulses, fades, or auto-scrolls.
- All actions are keyboard-reachable. Screen-reader support is bounded by what terminal emulators expose for TUIs — Grove does not introduce additional barriers but does not claim to be a screen-reader-optimized surface.
