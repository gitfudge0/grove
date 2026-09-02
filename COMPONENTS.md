# Grove component contract

Grove is a native Rust/GPUI worktree launchpad for AI coding agents and persistent embedded PTYs. This document is the implementation contract; `COMPONENTS.html` is the live specimen sheet. `DESIGN.md` and `DESIGN.html` own every value. Nothing here introduces a new value.

Global rules: controls use 4px radius and groups use 6px; depth comes from surface color and 1px seams; plum means brand/focus/active only; green means running/success; amber means needs-you/warning; red means destructive/error; violet means informational navigation. The terminal stays primary. Status always includes glyph plus word or count. Frame refs point to the planned hi-fi board.

## Actions

### Button

| Row | Content |
|---|---|
| **Purpose** | Commit a clear action without overpowering the terminal. |
| **Anatomy** | button → optional icon → label |
| **Variants** | plain, primary, danger; compact and regular |
| **States** | default, hover, pressed, focus-visible, disabled, loading |
| **Tokens** | color-brand, color-brand-hover, color-brand-pressed, color-destructive, color-text-primary, color-on-brand, control-h, radius-4, border-thin, space-8/12, duration-fast |
| **Used in planned frame refs** | A1 A2 A3 B1 B2 C1 C2 C3 D2 D3 E1 E2 |
| **Target framework** | `button()` or `div()` helper; semantic `c::*()` accessors and `rpx(CONTROL_H/RADIUS_4/SPACE_*)`. |

### IconButton

| Row | Content |
|---|---|
| **Purpose** | Expose compact chrome actions with an accessible name. |
| **Anatomy** | button → icon → optional state dot |
| **Variants** | plain, active, danger |
| **States** | default, hover, pressed, focus-visible, disabled, loading |
| **Tokens** | color-hover, color-focus, color-focus-wash, color-destructive, icon-14/16, control-h, radius-4, duration-fast |
| **Used in planned frame refs** | A1 A2 B1 B2 C1 C2 C3 D1 D3 E2 |
| **Target framework** | Reusable `icon_button()` returning `div()`; tooltip/accessibility label required; theme and `rpx()` tokens only. |

### AgentChoiceControl

| Row | Content |
|---|---|
| **Purpose** | Create a session and choose Claude Code, Codex, OpenCode, or Terminal. |
| **Anatomy** | trigger → current/default agent → chevron → anchored agent menu |
| **Variants** | default-agent split, explicit agent menu, no configured default |
| **States** | default, hover, open, focus-visible, disabled, loading, error |
| **Tokens** | color-brand, color-surface-raised, color-border, color-focus, control-h, radius-4/6, space-6/8/12 |
| **Used in planned frame refs** | C2 C3 |
| **Target framework** | `div()` trigger plus overlay menu; actions call existing spawn entities; `c::*()` and `rpx()`. |

### ShortcutHint

| Row | Content |
|---|---|
| **Purpose** | Show a discoverable keyboard path without competing with labels. |
| **Anatomy** | keycap → optional plus → keycap → action label |
| **Variants** | single key, chord, resize hint |
| **States** | default, muted, focus-context, disabled |
| **Tokens** | font-mono, text-micro, color-text-muted, color-surface-raised, color-border, radius-2, space-4 |
| **Used in planned frame refs** | A1 B1 B2 C1 C3 D1 D2 D3 E2 |
| **Target framework** | `keycap()` helper using `mono()`, `c::*()`, and `rpx(TEXT_MICRO/RADIUS_2)`. |

## Navigation & chrome

### WorkspaceRail

| Row | Content |
|---|---|
| **Purpose** | Keep workspace context persistent without becoming a content mode. |
| **Anatomy** | rail → workspace items → overflow cue → plus → flexible spacer → manage |
| **Variants** | one workspace, many workspaces, overflow |
| **States** | default, keyboard traversal, scrolled, Zen-hidden, loading, error |
| **Tokens** | color-bg-subtle, color-border, workspace-rail-w, gap-sm, space-6, z-sticky |
| **Used in planned frame refs** | A1 A2 A3 B1 B2 C1 C2 C3 D1 D2 D3 E1 E2 |
| **Target framework** | `workspace_rail()` composed from `div()` and Workspace entities; physical seam via `px(1.0)`. |

### WorkspaceRailItem

| Row | Content |
|---|---|
| **Purpose** | Select a workspace using generated monogram and non-color state cues. |
| **Anatomy** | item → active marker → monogram → needs-you count → tooltip |
| **Variants** | default, plus, manage, overflow cue |
| **States** | default, hover, keyboard focus, active, needs-you, disabled, loading, error |
| **Tokens** | color-surface-raised, color-hover, color-focus, color-selected-border, color-needs-you, control-h, radius-4, dot-6, text-micro |
| **Used in planned frame refs** | D1 D2 D3 E2 |
| **Target framework** | `workspace_rail_item()` with accessible name; fill plus 2px focus treatment; `c::*()` and `rpx()`. |

### AppBar / BreadcrumbContext

| Row | Content |
|---|---|
| **Purpose** | Carry global commands and preserve cross-workspace active-session context. |
| **Anatomy** | brand → workspace context → breadcrumb → spacer → global attention → settings |
| **Variants** | Monitor, Project, active PTY, in-another-workspace |
| **States** | default, loading context, cross-workspace, error |
| **Tokens** | color-bg-subtle, color-border, color-brand, color-text-muted, appbar-h, text-caption, tracking-brand, space-8/12 |
| **Used in planned frame refs** | A1 A2 A3 B1 B2 C1 C2 C3 E2 |
| **Target framework** | `app_bar()` plus breadcrumb helper; cross-workspace state reads active session and workspace IDs. |

### NavigationSidebar

| Row | Content |
|---|---|
| **Purpose** | Hold Monitor and Project plus destination-specific context. |
| **Anatomy** | workspace name → destinations → divider → filters or project context |
| **Variants** | Monitor context, Project context, collapsed at minimum width |
| **States** | default, loading, partial, error |
| **Tokens** | color-rail, color-border, context-sidebar-w, header-h, row-h, space-8/12, gap-sm |
| **Used in planned frame refs** | A1 A2 A3 B1 B2 C1 C2 C3 E1 E2 |
| **Target framework** | `navigation_sidebar()` using destination enum and workspace entity; width via `rpx(CONTEXT_SIDEBAR_W)`. |

### DestinationItem

| Row | Content |
|---|---|
| **Purpose** | Switch between the only primary destinations: Monitor and Project. |
| **Anatomy** | row → icon → label → optional shortcut |
| **Variants** | Monitor, Project |
| **States** | default, hover, active, focus-visible, disabled |
| **Tokens** | color-hover, color-selected, color-selected-border, color-brand, row-h, icon-16, radius-4, text-body-sm |
| **Used in planned frame refs** | A1 A2 A3 B1 B2 C1 C2 C3 E1 E2 |
| **Target framework** | `destination_item(ContentMode)`; selected state uses `c::SELECTED()` and `rpx(ROW_H)`. |

### ScopeSelector

| Row | Content |
|---|---|
| **Purpose** | Make Monitor scope explicit: Global or Selected workspace. |
| **Anatomy** | segmented group → Global → Selected workspace |
| **Variants** | global, selected workspace |
| **States** | default, hover, selected, focus-visible, disabled, loading |
| **Tokens** | color-surface, color-surface-raised, color-selected, color-selected-border, control-h, radius-4/6, text-caption |
| **Used in planned frame refs** | A1 A2 A3 B1 B2 |
| **Target framework** | `scope_selector()` bound to Monitor query state, not navigation mode. |

### ListGridToggle

| Row | Content |
|---|---|
| **Purpose** | Change presentation while preserving the same filtered collection. |
| **Anatomy** | segmented group → List icon+label → Grid icon+label |
| **Variants** | List, Grid |
| **States** | default, hover, selected, focus-visible, disabled |
| **Tokens** | color-selected, color-selected-border, color-brand, control-h, radius-4/6, icon-14 |
| **Used in planned frame refs** | A1 A2 A3 B1 B2 |
| **Target framework** | `list_grid_toggle()` bound to layout enum; Grid shares Monitor filters. |

### StatusBar

| Row | Content |
|---|---|
| **Purpose** | Persist running, backend, context, and grid resize hints. |
| **Anatomy** | status glyph+word/count → backend → context → spacer → shortcuts/version |
| **Variants** | normal, grid, grid-resize, terminal-panel |
| **States** | default, updating, partial, backend error |
| **Tokens** | color-bg-subtle, color-border, color-running, color-needs-you, color-text-muted, status-h, text-micro, font-mono, tracking-status |
| **Used in planned frame refs** | A1 A2 B1 B2 C1 C2 E2 |
| **Target framework** | `status_bar()` reads registry/backend/grid resize state; `mono()` with semantic glyph plus text. |

## Sessions & terminal

### SessionRow

| Row | Content |
|---|---|
| **Purpose** | Resume a session in one click with complete context. |
| **Anatomy** | state glyph+word → agent/task → project → worktree/branch → diff → actions |
| **Variants** | compact List, attention queue, selected workspace |
| **States** | default, hover, selected, working, needs-you, review, idle, exited, loading, partial, error |
| **Tokens** | color-hover, color-selected, color-selected-border, color-running, color-needs-you, color-text-primary/secondary/muted, row-h, radius-4 |
| **Used in planned frame refs** | A1 A2 A3 E1 E2 |
| **Target framework** | `session_row(SessionMeta)`; stable IDs drive selection; semantic state accessor, never color alone. |

### SessionStateGlyph / AttentionCount

| Row | Content |
|---|---|
| **Purpose** | Encode lifecycle and urgent counts with glyph plus word or count. |
| **Anatomy** | glyph or dot → state word/count |
| **Variants** | working, needs-you, review/done, idle, exited; global/workspace count |
| **States** | default, acknowledged, unread, loading, error |
| **Tokens** | color-running, color-needs-you, color-text-muted, color-destructive, dot-6/7/8, text-micro, font-mono |
| **Used in planned frame refs** | A1 A2 B1 B2 D1 E1 E2 |
| **Target framework** | `session_state()` and `attention_count()` helpers derived from activity store. |

### SessionTile

| Row | Content |
|---|---|
| **Purpose** | Supervise live sessions in Grid without losing terminal identity. |
| **Anatomy** | header → state → identity/context → diff/tools → clipped PTY → resize seam |
| **Variants** | selected, waiting, review, idle, exited |
| **States** | default, hover, focus, selected, loading PTY, partial metadata, error/exited |
| **Tokens** | color-terminal-bg/text, color-selected-border, color-amber-wash, color-needs-you, color-border, radius-4, grid-seam, header-h |
| **Used in planned frame refs** | B1 B2 |
| **Target framework** | `session_tile()` owns terminal entity view and grid focus; physical seam via `px(1.0)`. |

### SessionHeader / ToolCluster

| Row | Content |
|---|---|
| **Purpose** | Name the active session and expose supported actions. |
| **Anatomy** | running/state → project/worktree/task → path → spacer → run script → terminal panel → diff → zen → kill |
| **Variants** | agent, terminal, cross-workspace |
| **States** | default, action hover, run unavailable, loading, exited, error |
| **Tokens** | color-border, color-running, color-destructive, header-h, control-h, icon-14, text-body-sm, text-caption |
| **Used in planned frame refs** | A1 A2 B1 B2 C2 E2 |
| **Target framework** | `session_header()` plus compact action helpers; actions bind existing session/worktree commands. |

### TerminalSurface

| Row | Content |
|---|---|
| **Purpose** | Render the dominant persistent PTY without decorative UI typography. |
| **Anatomy** | terminal viewport → scrollback → selection → cursor → prompt |
| **Variants** | agent PTY, home terminal, panel terminal, tile preview |
| **States** | connecting, live, copy mode, loading, empty, partial, exited, error |
| **Tokens** | color-terminal-bg, color-terminal-text, color-text-muted, color-brand, font-mono, text-body, line-body, space-12/16 |
| **Used in planned frame refs** | A1 A2 B1 B2 C2 E2 |
| **Target framework** | Existing terminal entity/view; use `mono()`, theme terminal accessors, and token padding. |

### TerminalPanelDivider / ResizeHandle

| Row | Content |
|---|---|
| **Purpose** | Resize the optional terminal panel and Grid seams precisely. |
| **Anatomy** | 1px seam → hit target → focus indicator → resize cursor/hint |
| **Variants** | terminal panel vertical, grid horizontal/vertical |
| **States** | default, hover, dragging, keyboard resize, focus-visible, disabled |
| **Tokens** | color-border, color-border-strong, color-focus, grid-seam, control-h, duration-fast |
| **Used in planned frame refs** | B1 B2 C2 E2 |
| **Target framework** | `div()` seam with drag handlers; physical line `px(1.0)`, expanded invisible hit target via token control size. |

### DiffStatChip

| Row | Content |
|---|---|
| **Purpose** | Show added and removed counts without becoming a metric card. |
| **Anatomy** | add count → delete count |
| **Variants** | combined, add-only, delete-only, clean |
| **States** | default, loading, partial, error |
| **Tokens** | color-diff-add, color-diff-add-bg, color-diff-delete, color-diff-delete-bg, text-micro, font-mono, radius-2, space-4 |
| **Used in planned frame refs** | A1 A2 B1 C1 C2 E2 |
| **Target framework** | `diff_stat()` reads existing diff summary; `mono()` and `c::DIFF_*()`. |

## Projects & worktrees

### ProjectRow

| Row | Content |
|---|---|
| **Purpose** | Open project context and reveal its worktrees. |
| **Anatomy** | chevron → project name → activity count → row actions |
| **Variants** | compact, explorer |
| **States** | default, hover, expanded, active, loading, partial, error |
| **Tokens** | color-hover, color-selected, color-text-primary/muted, row-h, icon-12, text-body-sm, radius-4 |
| **Used in planned frame refs** | C1 C2 C3 D3 |
| **Target framework** | `project_row(Project)`; stable project ID, semantic selection, token row height. |

### WorktreeRow

| Row | Content |
|---|---|
| **Purpose** | Select a Git worktree and show branch/session context. |
| **Anatomy** | chevron → worktree name → branch/path → session count → actions |
| **Variants** | main, linked worktree, selected |
| **States** | default, hover, selected, dirty, loading, partial, missing/error |
| **Tokens** | color-hover, color-selected, color-selected-border, color-text-secondary/muted, row-h, font-mono, text-caption |
| **Used in planned frame refs** | C1 C2 C3 |
| **Target framework** | `worktree_row(Worktree)` from project tree entity; `mono()` for branch/path. |

### WorktreeDetailHeader

| Row | Content |
|---|---|
| **Purpose** | Anchor Project view around the selected worktree. |
| **Anatomy** | project breadcrumb → worktree/branch → path → diff → New session |
| **Variants** | main, linked, missing worktree |
| **States** | default, loading, partial, dirty, error |
| **Tokens** | color-surface, color-border, color-text-primary/secondary, color-diff-add/delete, header-h, text-title/caption |
| **Used in planned frame refs** | C1 C2 C3 |
| **Target framework** | `worktree_detail_header()` reads selected project/worktree and script availability. |

### NewSessionAction / AgentMenu

| Row | Content |
|---|---|
| **Purpose** | Spawn a supported agent in the current or chosen worktree. |
| **Anatomy** | New session trigger → anchored menu → agent rows → default marker → shortcut |
| **Variants** | current worktree, palette-selected worktree, default agent |
| **States** | default, hover, open, focus-visible, loading, no agents, partial, spawn error |
| **Tokens** | color-brand, color-surface-raised, color-border, color-focus, radius-4/6, control-h, row-h, space-8/12 |
| **Used in planned frame refs** | C1 C2 C3 |
| **Target framework** | `new_session_action()` uses existing spawn target/entity; anchored overlay and error feedback. |

## Inputs & overlays

### SearchField / FilterControl

| Row | Content |
|---|---|
| **Purpose** | Search sessions, projects, or worktrees and apply the agent/state filters shared by Monitor List and Grid. |
| **Anatomy** | search icon → text input → clear → shortcut; filter trigger → label/value → chevron → anchored options |
| **Variants** | session/project/workspace search, agent filter, state filter, combined filter |
| **States** | empty, focused, typing, default, hover, open, selected, loading, no matches, partial, disabled, error |
| **Tokens** | color-surface, color-border, color-selected, color-focus, color-text-primary/muted, control-h, radius-4, icon-14, text-caption, space-8 |
| **Used in planned frame refs** | A1 A2 A3 B1 B2 C1 D3 |
| **Target framework** | GPUI input bound to query state plus `filter_control()` updating the shared Monitor query entity; focus uses `c::FOCUS()`. |

### Tooltip / AnchoredPopover

| Row | Content |
|---|---|
| **Purpose** | Explain compact controls and host small contextual choices without a modal. |
| **Anatomy** | anchor → positioned surface → optional title → rows/actions → arrow/relationship |
| **Variants** | tooltip, menu, confirmation, agent menu |
| **States** | closed, delayed-open, open, focus-within, loading, error |
| **Tokens** | color-surface-raised, color-border-strong, color-text-primary/secondary, shadow-sm, radius-4/6, space-6/8/12, z-overlay |
| **Used in planned frame refs** | C3 D1 D2 D3 E1 |
| **Target framework** | GPUI overlay insertion order; anchor geometry from element bounds; tooltip carries accessible name. |

### WorkspaceCreatePopover

| Row | Content |
|---|---|
| **Purpose** | Create a workspace from the rail without gating onboarding. |
| **Anatomy** | plus anchor → name field → generated monogram → optional color later → create/cancel |
| **Variants** | new workspace, duplicate name |
| **States** | closed, open, typing, submitting, success, validation error |
| **Tokens** | color-surface-raised, color-border, color-focus, color-brand, radius-6, shadow-md, space-8/12/16, z-overlay |
| **Used in planned frame refs** | D2 |
| **Target framework** | Anchored overlay with WorkspaceRecord creation command; generated monogram in v1. |

### WorkspaceManageRow

| Row | Content |
|---|---|
| **Purpose** | Rename, reorder, move projects, or delete safely. |
| **Anatomy** | monogram → workspace name → project/session counts → menu actions |
| **Variants** | normal, active workspace, contains live/recovered sessions |
| **States** | default, hover, focus, editing, loading, partial, delete-blocked, error |
| **Tokens** | color-hover, color-selected, color-needs-you, color-destructive, row-h, radius-4, text-body-sm/caption |
| **Used in planned frame refs** | D3 |
| **Target framework** | `workspace_manage_row(WorkspaceRecord)`; delete command enforces project/live/recovered guards. |

## Recovery & feedback

### RecoveredSessionRow

| Row | Content |
|---|---|
| **Purpose** | Keep a rediscovered tmux session visible when hierarchy identity fails. |
| **Anatomy** | state → agent/task → saved path → failure reason → Reassign → Close |
| **Variants** | recoverable, path missing, metadata old |
| **States** | loading, partial, unassigned, reassigning, close-confirm, error |
| **Tokens** | color-amber-wash, color-needs-you, color-text-primary/secondary/muted, color-border, radius-4, row-h |
| **Used in planned frame refs** | E1 |
| **Target framework** | `recovered_session_row(DiscoveredSession)`; reads versioned tmux metadata and explicit recovery state. |

### EmptyState

| Row | Content |
|---|---|
| **Purpose** | Explain whether there is nothing yet or a filter found nothing. |
| **Anatomy** | display line → explanation → optional primary action → optional clear filters |
| **Variants** | nothing-yet, nothing-matched |
| **States** | default, loading, partial, error |
| **Tokens** | color-text-primary/secondary/muted, color-brand, text-display/body, space-12/16/24, icon-24 |
| **Used in planned frame refs** | A3 C1 E1 |
| **Target framework** | `empty_state(kind)` enum; no data vs no query matches remain distinct. |

### InlineError / Toast

| Row | Content |
|---|---|
| **Purpose** | Report local failure or transient completion without hiding terminal output. |
| **Anatomy** | status glyph+word → message → optional action/close |
| **Variants** | inline error, warning, success toast |
| **States** | enter, visible, action hover, dismissing, persistent error |
| **Tokens** | color-destructive, color-red-wash, color-needs-you, color-running, color-surface-raised, border-thin, radius-4/6, z-toast, duration-base |
| **Used in planned frame refs** | A3 C3 D2 D3 E1 E2 |
| **Target framework** | Inline `div()` or overlay entry; semantic status helper, motion honors reduced-motion setting. |
