# Grove 1.0.8

Changes since v1.0.2. The text-only launch carousel groups these into four highlights.

## Sessions and navigation

- Compact sidebar project and session navigation, hover project popups, collision-safe project initials, worktree grouping, tooltips, and inline launch controls.
- Clearer expanded project/worktree/session hierarchy, title-first session rows, activity icons, Git change counts, and working status.
- Sidebar collapse shortcut (`cmd+b` / `ctrl+b`); preserve list/grid view when switching workspaces.
- Restore terminal focus after closing a session; closing the last terminal no longer respawns it.
- Fix clipped launch controls and premature truncation in sidebar labels, grid headers, and command palette labels.
- More compact session lists, thin grid dividers, aligned popup separators, centered empty states, Geist UI text, and a lighter frosted macOS sidebar.

## Launching and shell environments

- Launch cross-project sessions with selected worktrees and shared context. Show their worktrees in navigation.
- Launch Terminal, Claude Code, Codex, or OpenCode from expanded worktree rows and compact project popups.
- Refresh shell environments periodically or on demand so new sessions pick up PATH changes.
- Allow spaces in worktree names.

## CLI and agent workflows

- CLI management for projects, workspaces, worktrees, sessions, scripts, session input/logs, and asynchronous removal status.
- Durable delegated task records with parent tasks, explicit results, inspection, and waiting; tmux sessions can reconnect after desktop restarts.
- Install or refresh the bundled Grove skill globally for Codex, Claude Code, and OpenCode from Settings. The CLI also supports explicit project scope.
- Install the Grove CLI alongside the desktop and keep it linked to the updated desktop executable. CLI control requires a running desktop on macOS/Linux; Windows CLI control is not supported yet.

## Updates and packaging

- Check the latest available release again before installing an update.
- Preserve a pending restart across update checks and offer an explicit restart action.
- Reduce release binaries and installer sizes across platforms.
- Refresh local macOS signing identity.

## Release preparation

- Bump Grove to 1.0.8 and enable its once-per-version text-only launch highlights. Retain the original 1.0.2 manifest entry and media for debug previews.
- Refresh release workflows and validation for CLI installation and complete runtime script output.
