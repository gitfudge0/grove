# Grove 1.0.12

## Native conversations

- Show work durations in minutes, including a clear subminute label.
- Keep conversation minimap markers compact, vertically centered, and aligned for easier navigation.
- Answer inline question lists and review confirmed answer receipts in the conversation.
- Navigate native conversations more easily, with turn timing and rendered Mermaid diagrams.
- Inspect clearer tool details and work disclosures, with tighter activity spacing and a shimmer limited to the latest work entry.

## Messages and composer

- Press Enter to queue a message while an agent works, or Cmd+Enter to steer the active turn.

# Grove 1.0.11

## Native agent conversations

- Run Codex and Claude Code in native chat sessions, with conversation history, tool activity, approval prompts, model controls, attachments, and session status inside Grove.
- Answer structured questions directly in chat, including choices, free text, numbers, secrets, and forms. Keep your composer draft while answering a question.

## Messages and composer

- Queue messages while an agent works, then edit or remove them in the pending message tray before they are sent.
- Refine chat bubbles, composer scrolling, and keyboard focus. Preserve manual scroll position while responses stream.
- Fix transcript text selection and Markdown artifact links in native conversations.

## Session navigation

- Refresh session and project navigation, including clearer native agent activity and question states.
- Keep the agent icon centered in the compact sidebar, with its working indicator at the lower-right corner.

# Grove 1.0.10

- Refresh stale worktree entries after a removal attempt finds the worktree is no longer listed by Git. Preserve sessions and unrelated folders when removal is rejected.

# Grove 1.0.9

## Terminal selection

- Restore ordinary drag selection in Claude Code and other terminal apps with mouse reporting enabled, while preserving clicks on app controls.
- Scroll terminal history while holding a selection near the top or bottom edge, including past the viewport, and extend the selected text for copying. Stop scrolling when the pointer leaves the edge or the drag ends.

# Grove 1.0.8

Changes since v1.0.2. The text-only launch carousel groups these into three highlights.

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

## Updates and packaging

- Check the latest available release again before installing an update.
- Preserve a pending restart across update checks and offer an explicit restart action.
- Reduce release binaries and installer sizes across platforms.
- Refresh local macOS signing identity.

## Release preparation

- Bump Grove to 1.0.8 and enable its once-per-version text-only launch highlights. Retain the original 1.0.2 manifest entry and media for debug previews.
- Refresh release workflows and validation for complete runtime script output.
