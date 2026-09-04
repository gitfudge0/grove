# Grove component contract

Grove is a native Rust and GPUI workspace for local Git projects, worktrees, AI coding sessions, and persistent PTYs. This document is the implementation contract. `COMPONENTS.html` is the live specimen sheet. `DESIGN.md` and `DESIGN.html` own every token; Phase 3 introduces no new values.

## Global rules

- Controls use radius-4, menus and cards use radius-8, dialogs use radius-12, and radius-full is reserved for status pills and indicators.
- Brand is near-white in dark mode and near-black in light mode. Violet is only agent identity, connection, focus, and active port. Green is running or success, amber is needs-you or warning, red is destructive or error, and blue is tertiary information.
- Build depth with neutral surface steps and borders before shadows.
- Project and List preserve the 236px hierarchy sidebar. Grid removes it so active sessions own the canvas.
- Worktree hover exposes three actual icon buttons for Codex, Claude Code, and Terminal. Each has an accessible name and matching tooltip.
- Header-level New session is a secondary action for unresolved context. It is never the dominant action.
- Session filtering is unsupported. Data-bearing components use loading, nothing yet, partial, and error.
- Frame references point to the corrected 43-frame `workspace-hierarchy-wireframe.html` board.

## Actions

### Button

| Row | Content |
|---|---|
| Purpose | Commits a named action with weight matched to consequence. |
| Anatomy | button → optional icon → label → optional progress |
| Variants | primary, secondary, ghost, danger; compact or regular |
| States | default, hover, pressed, focus-visible, disabled, loading |
| Tokens | control-h, radius-4, border-thin, color-inverse-cta-bg, color-inverse-cta-text, color-hover, color-destructive, color-focus, duration-fast |
| Used in frame refs | A5 B2 B4 B5 B6 B7 C1 C2 C3 C5 D3 D5 E5 E6 E7 G2 G4 G5 G7 |
| Target framework | `ModalBtn` with `modal_action` or `modal_action_sized`; body actions use `body_action`. |

### Icon button

| Row | Content |
|---|---|
| Purpose | Fits a single named tool into dense chrome. |
| Anatomy | button → currentColor SVG icon → tooltip |
| Variants | flat, outlined, active, danger |
| States | default, hover, pressed, focus-visible, disabled, loading |
| Tokens | control-h, icon-12, icon-14, icon-16, radius-4, color-text-secondary, color-hover, color-focus, color-destructive |
| Used in frame refs | D1 D2 D6 E1 E2 E4 E5 E6 E7 F1 F2 F3 F4 G2 |
| Target framework | `icon_btn` or `flat_icon_btn`, with `hint_tooltip` and a unique GPUI element id. |

### Direct-start agent trio

| Row | Content |
|---|---|
| Purpose | Starts an agent directly in the hovered worktree. |
| Anatomy | hover action strip → Codex icon button → Claude Code icon button → Terminal icon button |
| Variants | concealed, revealed, revealed with tooltip |
| States | default, hover, focus-visible, unavailable, starting, error |
| Tokens | control-h, icon-14, gap-4, radius-4, color-text-muted, color-hover, color-accent, color-focus |
| Used in frame refs | D6 E1 E2 |
| Target framework | `rows::render_row` worktree action strip using three `icon_btn` calls and `hint_tooltip`; dispatch `RowAction::SpawnAgent`. |

### Project/List/Grid switch

| Row | Content |
|---|---|
| Purpose | Changes the workspace view without changing its selection. |
| Anatomy | segmented group → Project button → List button → Grid button |
| Variants | Project active, List active, Grid active |
| States | default, hover, pressed, focus-visible, active, disabled |
| Tokens | control-h, radius-4, border-thin, color-border, color-selected, color-text-primary, color-text-secondary, color-focus |
| Used in frame refs | F1 F2 F3 F4 F5 |
| Target framework | `seg_group` with three `seg_button` or `seg_button_content` children; existing row and grid actions supply dispatch. |

## Navigation and chrome

### Appbar

| Row | Content |
|---|---|
| Purpose | Holds brand, workspace context, view controls, and compact global tools. |
| Anatomy | strip → GROVE → workspace trigger → optional view switch → spacer → app tools → bottom border |
| Variants | Project/List shell, full-canvas Grid shell, loading target |
| States | default, loading, partial, error |
| Tokens | appbar-h, color-bg-subtle, color-brand, color-border-strong, text-15, space-12 |
| Used in frame refs | A1-A5 B1-B7 C1-C5 D1-D6 E1-E7 F1-F5 G1-G8 |
| Target framework | Extend `appbar::appbar`; the workspace trigger and view switch are proposed appbar compositions, not existing helpers. |

### Workspace trigger

| Row | Content |
|---|---|
| Purpose | Names the active workspace and opens switching. |
| Anatomy | outlined button → workspace label → down chevron |
| Variants | compact, long-name truncated |
| States | default, hover, focus-visible, expanded, loading, error |
| Tokens | control-h, radius-4, border-thin, color-border, color-text-primary, color-hover, color-focus |
| Used in frame refs | A1-A5 B1-B7 C1-C5 D1-D6 E1-E7 F1-F5 G1-G8 |
| Target framework | Proposed composition inside `appbar::appbar` using `div().id(...)`, border tokens, and app-level workspace dispatch. |

### Workspace dropdown

| Row | Content |
|---|---|
| Purpose | Switches workspace and keeps create/manage actions pinned. |
| Anatomy | anchored panel → title → workspace buttons → divider → pinned Create workspace → pinned Manage workspaces |
| Variants | selected row, waiting badge, overflow scroll, recoverable error |
| States | loading, nothing yet, partial, error, open, keyboard focus |
| Tokens | radius-8, border-thin, shadow-md, color-surface-raised, color-border, color-selected, color-needs-you, color-destructive, z-overlay |
| Used in frame refs | A2 A4 B1 G1 G6 G7 G8 |
| Target framework | Proposed `gpui::deferred(gpui::anchored())` appbar composition; rows follow `click_row_on` with pinned Create and Manage. |

### Project sidebar

| Row | Content |
|---|---|
| Purpose | Shows the selected workspace as Project, Worktree, Session hierarchy or scoped List. |
| Anatomy | 236px panel → view controls → secondary New session → hierarchy or status groups |
| Variants | Project tree, List groups, collapsed project |
| States | loading, nothing yet, partial, error, selected, collapsed |
| Tokens | sidebar-w, row-h, color-surface, color-border, color-selected, color-text-primary, color-text-muted |
| Used in frame refs | A1-A5 B1-B7 C1-C5 D1-D6 E1-E7 F1-F2 F4-F5 G1-G8 |
| Target framework | `Sidebar::render`, with visible rows flattened and painted by `rows::render_row`. Grid omits this component. |

### Session header

| Row | Content |
|---|---|
| Purpose | Keeps selected session identity and tools above its PTY. |
| Anatomy | strip → agent icon → label → branch/context → diff → divider → tool cluster → bottom border |
| Variants | running, needs-you, failed, branchless terminal |
| States | loading, partial, error, active |
| Tokens | header-h, color-bg-subtle, color-border, color-text-primary, color-text-secondary, color-running, color-needs-you, color-destructive |
| Used in frame refs | A1 A4 E4 E5 E6 E7 F1 F2 F4 F5 G3 G7 |
| Target framework | `session_header::session_header` with `SessionHeaderData` and optional `ToolCluster`. |

### Statusbar

| Row | Content |
|---|---|
| Purpose | Reports running count, backend, theme, transient status, hints, and version. |
| Anatomy | top border → running group → labels → toast slot → spacer → hints → version |
| Variants | normal, grid resize, transient info/error |
| States | loading, nothing yet, partial, error |
| Tokens | status-h, color-bg-subtle, color-border, color-text-muted, color-running, color-tertiary, color-destructive, text-10 |
| Used in frame refs | A1-A5 B1-B7 C1-C5 D1-D6 E1-E7 F1-F5 G1-G8 |
| Target framework | `statusbar::statusbar(StatusbarCtx)`; messages occupy its existing toast slot. |

## Containers and overlays

### Scrim

| Row | Content |
|---|---|
| Purpose | Blocks background interaction behind a modal. |
| Anatomy | full-window overlay → centered dialog |
| Variants | centered, top-drop |
| States | entering, open, exiting, reduced motion |
| Tokens | color-scrim, z-modal, duration-fast |
| Used in frame refs | B2 B3 B4 B5 B7 C2 C3 C5 D3 D4 D5 E7 G2 G4 G5 |
| Target framework | `scrim` or `scrim_top_drop`; paint after normal content. |

### Dialog

| Row | Content |
|---|---|
| Purpose | Contains focused create, edit, confirm, or recovery work. |
| Anatomy | panel → header → body → inline feedback → footer |
| Variants | form, progress, error, confirmation, wide manager |
| States | default, loading, nothing yet, partial, error, destructive confirmation |
| Tokens | radius-12, color-surface-raised, color-border, shadow-lg, space-24, gap-12 |
| Used in frame refs | B2 B3 B4 B5 B7 C2 C3 C5 D3 D4 D5 E7 G2 G4 G5 |
| Target framework | `modal_panel`, `modal_header_slotted`, `modal_body`, and `modal_footer`; buttons use `ModalBtn`. |

### Anchored menu/popover

| Row | Content |
|---|---|
| Purpose | Places contextual actions next to their trigger. |
| Anatomy | anchor → deferred panel → optional title → button rows → divider/footer |
| Variants | workspace dropdown, project actions, tooltip |
| States | open, hover, keyboard focus, loading, error, dismissed |
| Tokens | radius-8, color-surface-raised, color-border, shadow-md, z-overlay, space-8 |
| Used in frame refs | A2 A4 B1 D2 E2 G1 G6 G7 G8 |
| Target framework | `gpui::deferred(gpui::anchored())`; rows follow `click_row_on`, tooltips use `hint_tooltip`. |

## Workspace/project hierarchy

### Disclosure

| Row | Content |
|---|---|
| Purpose | Expands or collapses project and worktree descendants. |
| Anatomy | icon button → right/down chevron → row label |
| Variants | project, worktree |
| States | collapsed, expanded, hover, focus-visible, disabled |
| Tokens | icon-12, control-h, color-text-muted, color-hover, color-focus |
| Used in frame refs | A1 A4 C4 D1 D6 E1 E4 F1 F4 F5 G3 |
| Target framework | The chevron and selection handlers inside `rows::render_row`. |

### Project row

| Row | Content |
|---|---|
| Purpose | Represents a local Git project and its rollup actions. |
| Anatomy | disclosure → project label → status/count → spacer → worktree action → settings/remove |
| Variants | expanded, collapsed, non-Git error |
| States | loading, nothing yet, partial, error, hover, selected |
| Tokens | row-h, color-text-primary, color-text-muted, color-running, color-destructive, color-hover |
| Used in frame refs | A1 A4 C4 D1 D2 D6 E1 E4 F1 F4 F5 G3 |
| Target framework | `rows::render_row(TreeRow::Project, RowCtx)` and its internal project row composition. |

### Worktree row

| Row | Content |
|---|---|
| Purpose | Represents one checkout and reveals direct-start agents on hover. |
| Anatomy | disclosure → worktree name → branch → spacer → main tag or direct-start trio |
| Variants | main, branch, zero sessions, expanded, hovered |
| States | loading, nothing yet, partial, error, selected, hover |
| Tokens | row-h, radius-4, color-text-secondary, color-text-muted, color-running, color-hover, gap-4 |
| Used in frame refs | A1 A4 C4 D1 D6 E1 E2 E3 E4 F1 F4 F5 G3 |
| Target framework | `rows::render_row(TreeRow::Worktree, RowCtx)`; `hovered_wt` reveals three `icon_btn` actions. |

### Session row

| Row | Content |
|---|---|
| Purpose | Opens one session and states its process condition without relying on color. |
| Anatomy | state glyph → agent/task label → context → spacer → close/retry action |
| Variants | working, needs-you, done, idle, exited, starting |
| States | loading, nothing yet, partial, error, hover, selected, pending close |
| Tokens | row-h, color-selected, color-running, color-needs-you, color-destructive, color-text-muted, icon-14 |
| Used in frame refs | A1 A4 E3 E4 E5 E6 E7 F1 F2 F4 F5 G3 G7 |
| Target framework | `rows::render_row(TreeRow::Session, RowCtx)` with `state_glyph`; selection and kill use `RowAction`. |

## Sessions and terminal

### Full-canvas active-session grid

| Row | Content |
|---|---|
| Purpose | Gives all active sessions the canvas while Grid is selected. |
| Anatomy | appbar with view switch → tile columns → resize seams → statusbar |
| Variants | one tile, two tiles, three or more tiles |
| States | loading, nothing yet, partial, error, resizing |
| Tokens | color-bg, color-border, color-divider-soft, gap-2, duration-fast |
| Used in frame refs | F3 |
| Target framework | `grid::grid(GridCtx)` below the persistent appbar; do not render `Sidebar` in this mode. |

### Active-session tile

| Row | Content |
|---|---|
| Purpose | Keeps one agent, task, workspace path, status, and clipped PTY together. |
| Anatomy | tile → session header → context line → clipped PTY |
| Variants | Codex working, Claude Code needs-you, Terminal running |
| States | loading, partial, error, focused, resizing |
| Tokens | radius-8, color-surface, color-border, color-focus, color-accent, color-running, color-needs-you, color-terminal-bg |
| Used in frame refs | F3 |
| Target framework | Tile composition inside `grid::grid` from `TileData`; use existing tile focus and resize dispatch. |

### Workspace session list/card

| Row | Content |
|---|---|
| Purpose | Groups sessions in the selected workspace by Needs you, Working, and Idle. |
| Anatomy | status group header → session cards → retained selection |
| Variants | Needs you, Working, Idle groups |
| States | loading, nothing yet, partial, error, selected |
| Tokens | radius-8, color-surface, color-border, color-selected, color-needs-you, color-running, color-text-muted |
| Used in frame refs | F2 |
| Target framework | `Sidebar::render` in flat session mode using `rows::flatten_sessions` and `rows::render_row(TreeRow::SessionCard, RowCtx)`. |

### PTY

| Row | Content |
|---|---|
| Purpose | Displays and accepts terminal process output for the selected session. |
| Anatomy | terminal surface → output lines → prompt/caret |
| Variants | agent PTY, Terminal PTY, clipped tile excerpt |
| States | loading, nothing yet, partial, error, running, awaiting input, exited |
| Tokens | color-terminal-bg, color-terminal-text, font-mono, text-12, color-running, color-needs-you, color-destructive |
| Used in frame refs | A1 A4 E3 E4 E5 E6 E7 F1 F2 F3 F4 F5 G3 G7 |
| Target framework | Existing terminal entity/view below `session_header`; Grid embeds the same session view through `TileData`. |

## Lists and rows

### Status group header

| Row | Content |
|---|---|
| Purpose | Labels a scoped session group and its count. |
| Anatomy | mono uppercase label → count chip → optional activity indicator |
| Variants | Needs you, Working, Idle, Terminals |
| States | loading, nothing yet, partial, error, expanded, collapsed |
| Tokens | row-h, font-mono, text-10, color-text-muted, color-needs-you, color-running, radius-full |
| Used in frame refs | F2 |
| Target framework | `rows::render_row(TreeRow::SectionHeader, RowCtx)`; terminal variant maps to `rows::terminals_header`. |

## Inputs

### Text/path field

| Row | Content |
|---|---|
| Purpose | Captures workspace names and local Git paths without losing edits. |
| Anatomy | label → field shell → input → optional help/error |
| Variants | text, local path, confirmation text, locked progress |
| States | default, focus-visible, filled, disabled, loading, error |
| Tokens | control-h, radius-4, font-mono, text-12, color-surface, color-border, color-focus, color-text-primary |
| Used in frame refs | B2 B3 B4 B5 B7 C2 C3 C5 D3 D4 D5 G2 G5 |
| Target framework | `ModalInput::single_line` inside `field_box`; host modal owns focus policy and validation. |

### Inline validation

| Row | Content |
|---|---|
| Purpose | Explains a local field error while preserving all values. |
| Anatomy | field → error text → recovery action when needed |
| Variants | required, duplicate, non-Git, branch conflict, path conflict, backend failure |
| States | hidden, visible, updating, resolved |
| Tokens | text-11, color-destructive, color-text-secondary, gap-4 |
| Used in frame refs | B3 B7 C5 D4 E6 G4 G7 |
| Target framework | `note_text` beside `ModalInput` or `field_box`; the modal entity keeps the error until the next edit. |

## Progress and feedback

### Empty state

| Row | Content |
|---|---|
| Purpose | Explains what the selected workspace lacks and offers its next valid action. |
| Anatomy | title → scoped explanation → optional primary setup action |
| Variants | no projects, no sessions, no active Grid sessions |
| States | loading handoff, nothing yet, partial, error |
| Tokens | text-24, text-13, color-text-primary, color-text-secondary, space-24, gap-8 |
| Used in frame refs | A5 B6 C1 C4 D6 E1 F3 |
| Target framework | `grid::empty_state` for canvas; sidebar empty rows use `rows::render_row(TreeRow::Empty, RowCtx)`. |

### Progress/skeleton

| Row | Content |
|---|---|
| Purpose | Holds layout steady while switching or creating local resources. |
| Anatomy | reserved row or panel → neutral bars → concise progress label |
| Variants | sidebar skeleton, content progress, locked dialog progress |
| States | loading, partial, success handoff, error |
| Tokens | radius-4, color-surface, color-selected, color-text-muted, duration-slow, opacity-muted |
| Used in frame refs | A3 B5 C3 D5 E3 |
| Target framework | Plain child `div()` progress/skeleton elements in the owning view; reduced motion uses `Duration::ZERO`. |

### Status chip/banner

| Row | Content |
|---|---|
| Purpose | Labels status or explains feedback with words and a non-color cue. |
| Anatomy | optional glyph/dot → label → optional action |
| Variants | selected, waiting, running/success, warning, error, info |
| States | loading, nothing yet, partial, error, dismissed |
| Tokens | radius-full for chips, radius-8 for banners, color-running, color-needs-you, color-destructive, color-tertiary, color-text-primary |
| Used in frame refs | A2 A4 B3 B7 C5 D4 E3 E4 E5 E6 G4 G6 G7 |
| Target framework | `status_dot`, `status_dot_hollow`, `keycap_filled`, or a token-only `div()` banner; status text names the condition. |
