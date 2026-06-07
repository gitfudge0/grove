# Grove Design System

description: Desktop worktree launchpad for AI coding agents with embedded PTY sessions.

## Product Surface

Grove is a native desktop app for supervising projects, worktrees, and live agent sessions in one persistent window. The app chrome stays quiet so the embedded PTY output remains the primary content.

The interface has three sidebar views:

- **tree**: projects, worktrees, and sessions in their repository hierarchy.
- **activity**: all sessions grouped by running, idle, and worktrees with no sessions.
- **terminal**: persistent home shells rooted at `~`.

The session workspace renders the active agent as a PTY canvas. When needed, a worktree shell can open as a right-docked panel beside the agent.

## Principles

1. **Sessions are the unit of work.** Projects and worktrees exist to create, find, and return to sessions.
2. **Show state, not chrome.** Running state is compact: a green dot plus count. Avoid dashboard metrics, oversized badges, and decorative status cards.
3. **Keyboard and mouse both matter.** Common actions must be reachable by keyboard, while row controls and toolbar buttons remain visible and predictable.
4. **Theme is semantic.** Theme colors describe roles (`bg`, `fg`, `comment`, `green`, `red`) instead of fixed component colors.
5. **PTY content is sovereign.** Grove hosts terminal applications; it should not restyle their text beyond faithful ANSI rendering.

## Color Roles

The shared theme model exposes eleven semantic colors:

| Role | Use |
|---|---|
| `bg` | Workspace canvas and base surface |
| `bg_highlight` | Active rows, selected controls, focused state |
| `fg` | Primary text and active icons |
| `fg_dark` | Secondary text |
| `comment` | Muted text and low-priority metadata |
| `blue` | Navigation and informational accents |
| `cyan` | Secondary accent |
| `magenta` | Agent/category accent |
| `green` | Running state and positive actions |
| `yellow` | Warnings and pending state |
| `red` | Destructive actions and errors |

The GUI derives additional surface tokens in `src/gui/palette.rs`: rail, strip, hover, border, soft border, and foreground steps. Those derived colors should remain subtle and tied to the active theme.

## Typography

Grove uses a deliberately small type system:

- UI text: IBM Plex Sans.
- PTY text: Blex Mono Nerd Font Mono.
- Row labels: compact, medium weight, clipped rather than wrapped.
- Metadata: smaller and muted.
- No decorative italics.

Large display type is not part of the app surface. Grove is a workbench, not a landing page.

## Layout

- Sidebar width should stay stable enough for project scanning.
- Rows are dense, with hover controls revealed without shifting layout.
- Cards are reserved for repeated items, modals, and framed tool surfaces. Do not nest cards.
- The PTY canvas should occupy the dominant workspace area.
- The right-docked terminal panel should feel like an operational split, not a modal.

## Interaction

- Row actions should use compact icon or short-label buttons.
- Theme, tmux, and confirmation flows use modals.
- Text selection in PTYs should behave like a terminal: drag selects, copy reads the selected cells, paste forwards to the focused PTY.
- Zen mode may hide chrome to give the active PTY more room.

## Accessibility

- Color is never the only carrier of session state; the running dot is paired with a count.
- Focus and selection must have visible background/border treatment.
- Text must remain readable across all bundled light and dark themes.
- Motion should be functional and restrained.

## Anti-Patterns

- Marketing hero layouts inside the app.
- Decorative gradients, glow, glass, and status-card dashboards.
- UI chrome that competes with PTY output.
- Reflowing row heights when hover actions appear.
- Treating embedded terminals as screenshots instead of live, selectable PTYs.
