# Grove component contract

Grove is a native Rust and GPUI workspace for local Git projects, worktrees, AI coding sessions, and persistent PTYs. This document is the implementation contract. `COMPONENTS.html` is the live specimen sheet. `DESIGN.md` and `DESIGN.html` own every token; this handoff introduces no new visual values. `COMPONENTS.md`/`COMPONENTS.html` own component anatomy and states; `screens.html` owns the 45-frame flow and composition inventory.

The Rust/GPUI entries below distinguish existing integration points from proposed adaptations. All 34 catalog IDs have a mirrored implementation map below and in `COMPONENTS.html`. Source files identify existing owning modules; a proposed composition inside one of those modules is not an existing helper. This preparation changes reference documents only; it does not implement or compile the native overhaul.

## Global rules

- Form wells, regular form actions, and selection tiles use radius-12; grouped panels use radius-16; compact actions and menus use radius-8. Radius-full is reserved for switches, pills, and indicators.
- Brand and focus are near-white in dark mode and near-black in light mode. Violet is agent identity, connection, and active port. Green is running, success, or an enabled switch; amber is needs-you or warning, red is destructive or error, and blue is tertiary information.
- Use flat neutral surfaces and quiet borders with shadow-none.
- Project and List preserve the 236px hierarchy sidebar. Grid removes it so active sessions own the canvas.
- Worktree hover exposes three actual icon buttons for Codex, Claude Code, and Terminal. Each has an accessible name and matching tooltip.
- Session creation exists only on worktree hover or keyboard focus through labeled Codex, Claude Code, and Terminal launch icons. Headers and empty states never expose New session.
- Session filtering is unsupported. Data-bearing components use loading, nothing yet, partial, and error.
- Frame references point to the 45-frame `screens.html` inventory. The older `workspace-hierarchy-wireframe.html` is historical context, not the current frame authority.
- Every action button contains a semantic currentColor SVG from the DESIGN.md catalog. Text actions lead with the icon by default; disclosure and navigation may trail. Icon-only actions require an accessible name and tooltip. Switches and selection tiles express state directly and do not receive action icons.

## Actions

### Button

| Row | Content |
|---|---|
| Purpose | Commits a named action with weight matched to consequence. |
| Anatomy | button → required semantic currentColor SVG in a stable leading slot → label → optional progress |
| Variants | primary, secondary, ghost, danger; compact or regular |
| States | default, hover, pressed, focus-visible, disabled, loading |
| Tokens | form-button-h, control-h, radius-12, radius-8, border-thin, color-inverse-cta-bg, color-inverse-cta-text, color-hover, color-destructive, color-focus, duration-fast |
| Used in frame refs | A5 B2 B4 B5 B6 B7 C1 C2 C3 C5 D3 D5 E5 E6 E7 G2 G4 G5 G7 |
| Target framework | `button` · **adapt** · ModalBtn + adapted modal_action/modal_action_sized/body_action. Source: `src/views/components.rs`. |

Every action button maps to the DESIGN.md icon catalog. Text buttons lead with an icon by default; only disclosure and forward navigation trail with a directional icon. Icon-only buttons require an `aria-label` and tooltip. Loading swaps or animates the leading icon without collapsing its slot, and disabled controls retain it.

GPUI implementation constraint: Keep semantic icon and progress in one reserved leading slot; regular actions 44px, compact 24px. Existing text-only mouse-down helpers need focus-visible, keyboard/on_click, disabled dispatch guards and stable loading geometry.

### Icon button

| Row | Content |
|---|---|
| Purpose | Fits a single named tool into dense chrome. |
| Anatomy | button → currentColor SVG icon → tooltip |
| Variants | flat, outlined, active, danger |
| States | default, hover, pressed, focus-visible, disabled, loading |
| Tokens | control-h, icon-12, icon-14, icon-16, radius-8, color-text-secondary, color-hover, color-focus, color-destructive |
| Used in frame refs | D1 D2 D6 E1 E2 E4 E5 E6 E7 F1 F2 F3 F4 G2 |
| Target framework | `icon` · **adapt** · Adapted icon_btn/flat_icon_btn + hint_tooltip. Source: `src/views/components.rs`. |

GPUI implementation constraint: Fixed 24px action box, semantic SVG, accessible name and matching tooltip. Add role, focus and keyboard activation; retain nested-row propagation protection. Tooltip alone does not name the control.

### Direct-start agent trio

| Row | Content |
|---|---|
| Purpose | Starts an agent directly in the hovered worktree. |
| Anatomy | hover action strip → Codex icon button → Claude Code icon button → Terminal icon button |
| Variants | concealed, revealed, revealed with tooltip |
| States | default, hover, focus-visible, unavailable, starting, error |
| Tokens | control-h, icon-14, gap-4, radius-8, color-text-muted, color-hover, color-accent, color-focus |
| Used in frame refs | D6 E1 E2 |
| Target framework | `trio` · **adapt** · Worktree-local Codex/Claude Code/Terminal actions via RowAction::SpawnAgent. Source: `src/views/rows.rs`. |

GPUI implementation constraint: Reveal three fixed launch slots on hover OR keyboard focus without shifting labels; name each agent and worktree. Current available-agent loop also exposes Run/More/Delete; unavailable/starting/error states need explicit treatment.

### Project/List/Grid switch

| Row | Content |
|---|---|
| Purpose | Changes the workspace view without changing its selection. |
| Anatomy | borderless 24x24 Project/List toggle showing the destination icon → 1-token gap → borderless 24x24 Grid button |
| Variants | Project active shows List destination; List active shows Project destination; Grid active remembers and shows the last non-grid destination |
| States | default, hover, pressed, focus-visible, active, disabled |
| Tokens | control-h, icon-14, gap-2, radius-8, color-hover, color-selected, color-text-primary, color-text-secondary, color-focus |
| Used in frame refs | F1 F2 F3 F4 F5 |
| Target framework | `switch` · **add** · Destination toggle + Grid, placed by screens composition. Source: `src/views/appbar.rs`. |

## Navigation and chrome

GPUI implementation constraint: Two separate borderless 24px buttons with 2px gap, labels/tooltips and Grid toggled state. screens.html places them in sidebar side-head for Project/List and appbar for Grid, never both. Preserve selection and remembered non-grid destination; joined seg_group is not the target.

### Appbar

| Row | Content |
|---|---|
| Purpose | Holds brand, workspace context, view controls, and compact global tools. |
| Anatomy | full-width strip → GROVE → independent workspace control → Grid-mode view controls → spacer → app tools → bottom border; Project/List view controls live in sidebar side-head |
| Variants | Project active, List active, Grid active; workspace loading and retry leave view controls unchanged |
| States | default, loading, partial, error |
| Tokens | appbar-h, color-bg-subtle, color-brand, color-border-strong, text-15, space-12 |
| Used in frame refs | A1-A5 B1-B7 C1-C5 D1-D6 E1-E7 F1-F5 G1-G8 |
| Target framework | `appbar` · **adapt** · Adapted appbar::appbar with workspace trigger and Grid-mode view controls. Source: `src/views/appbar.rs`. |

GPUI implementation constraint: Stable full-width brand/workspace/tools bar. Follow screens.html placement: view controls live here only in Grid, and in sidebar side-head for Project/List. COMPONENTS appbar specimen differs; preserve its styling, not duplicate controls. Workspace loading/retry must not move unrelated navigation.

### Workspace trigger

| Row | Content |
|---|---|
| Purpose | Names the active workspace and opens switching. |
| Anatomy | 24px leading slot → flexible workspace label/meta → down chevron in 28px trailing slot |
| Variants | normal, long, loading, and retry; the two view controls remain independent and use the mode-specific placement in screens.html |
| States | default, hover, focus-visible, expanded, loading, error |
| Tokens | control-h, radius-8, border-thin, color-border, color-text-primary, color-hover, color-focus |
| Used in frame refs | A1-A5 B1-B7 C1-C5 D1-D6 E1-E7 F1-F5 G1-G8 |
| Target framework | `trigger` · **add** · Proposed workspace trigger inside appbar::appbar. Source: `src/views/appbar.rs`. |

GPUI implementation constraint: 24px leading slot, min-width-zero flexible label/meta, 28px chevron slot. Named keyboard-operable trigger exposes expanded state; keep source bounds and focus handle for dismissal return.

### Workspace dropdown

| Row | Content |
|---|---|
| Purpose | Switches workspace and keeps create/manage actions pinned. |
| Anatomy | anchored panel → title → scroll region → 24px leading slot → flexible label/meta → fixed 48px status/count → fixed 28px trailing slot → pinned actions |
| Variants | selected row, waiting badge, overflow scroll, recoverable error |
| States | loading, nothing yet, partial, error, open, keyboard focus |
| Tokens | radius-8, border-thin, shadow-none, color-surface-raised, color-border, color-selected, color-needs-you, color-destructive, z-overlay |
| Used in frame refs | A2 A4 B1 G1 G6 G7 G8 |
| Target framework | `workspace-menu` · **add** · Proposed anchored workspace dropdown with pinned actions. Source: `src/views/appbar.rs`. |

GPUI implementation constraint: Title and Create/Manage are siblings of a constrained scrolling row region. Rows use 24/flexible/48/28 slots, named selected state, keyboard navigation and visible loading/empty/partial/error; Escape/outside click restores focus.

### Project sidebar

| Row | Content |
|---|---|
| Purpose | Shows the selected workspace as Project, Worktree, Session hierarchy or scoped List. |
| Anatomy | 236px panel → two-button view control → hierarchy or status groups → worktree-local agent launch affordances |
| Variants | Project tree, List groups, collapsed project |
| States | loading, nothing yet, partial, error, selected, collapsed |
| Tokens | sidebar-w, row-h, color-surface, color-border, color-selected, color-text-primary, color-text-muted |
| Used in frame refs | A1-A5 B1-B7 C1-C5 D1-D6 E1-E7 F1-F2 F4-F5 G1-G8 |
| Target framework | `sidebar` · **adapt** · Adapted Sidebar::render + rows::render_row. Source: `src/views/sidebar.rs`. |

GPUI implementation constraint: Project and List retain 236px hierarchy sidebar with two view controls in side-head per screens.html; Grid omits sidebar and places controls in appbar. Preserve scroll/selection and collapsed semantics; reveal worktree launch on keyboard focus. No duplicate view controls or session filtering.

### Session header

| Row | Content |
|---|---|
| Purpose | Keeps selected session identity and tools above its PTY. |
| Anatomy | strip → agent icon → label → branch/context → diff → divider → tool cluster → bottom border |
| Variants | running, needs-you, failed, branchless terminal |
| States | loading, partial, error, active |
| Tokens | header-h, color-bg-subtle, color-border, color-text-primary, color-text-secondary, color-running, color-needs-you, color-destructive |
| Used in frame refs | A1 A4 E4 E5 E6 E7 F1 F2 F4 F5 G3 G7 |
| Target framework | `session-header` · **adapt** · Adapted session_header::session_header(SessionHeaderData, ToolCluster). Source: `src/views/session_header.rs`. |

GPUI implementation constraint: Session identity and context precede diff and named icon tools. Reuse dispatch; apply header geometry and words/non-color cues for loading, running, needs-you, failed and branchless terminal.

### Statusbar

| Row | Content |
|---|---|
| Purpose | Reports running count, backend, theme, transient status, hints, and version. |
| Anatomy | top border → running group → labels → toast slot → spacer → hints → version |
| Variants | normal, grid resize, transient info/error |
| States | loading, nothing yet, partial, error |
| Tokens | status-h, color-bg-subtle, color-border, color-text-muted, color-running, color-tertiary, color-destructive, text-10 |
| Used in frame refs | A1-A5 B1-B7 C1-C5 D1-D6 E1-E7 F1-F5 G1-G8 |
| Target framework | `statusbar` · **adapt** · Adapted statusbar::statusbar(StatusbarCtx). Source: `src/views/statusbar.rs`. |

## Containers and overlays

GPUI implementation constraint: Running group, backend/theme, existing toast slot, spacer, hints/version. Apply tokens and explicit status words; actionable retries need named keyboard-operable controls, while passive messages do not steal focus.

### Scrim

| Row | Content |
|---|---|
| Purpose | Makes the underlying app inert while keeping the initiating row or tile legible. |
| Anatomy | visible mini app context → low-opacity neutral veil → crisp focus aperture at source → edge-attached confirmation tray |
| Variants | selected session row, active-session tile, workspace manager row |
| States | entering, open, cancel, confirm, exiting, reduced motion |
| Tokens | color-scrim at opacity-pressed, color-focus, border-thin, color-border, z-overlay, z-modal, duration-fast |
| Used in frame refs | B2 B3 B4 B5 B7 C2 C3 C5 D3 D4 D5 E7 G2 G4 G5 |
| Target framework | `scrim` · **adapt** · Proposed neutral veil + source repaint + anchored tray in ModalLayer. Source: `src/views/modals/mod.rs`. |

GPUI implementation constraint: Current scrim centers content and is not final presentation. Paint low-opacity veil only, then crisp initiating row/tile and edge tray; block underlying PTY mouse/keyboard interaction, trap decision focus and restore it on Cancel/Escape.

### Dialog

| Row | Content |
|---|---|
| Purpose | Keeps create, close, and delete decisions attached to their initiating workspace or session context. |
| Anatomy | quick-create: switcher trigger → shared-edge popover → icon-led field → embedded submit and Cancel; session close: source row or tile → shared-edge tray → inline key-value consequences → actions; delete: manager row → in-place danger confirmation row |
| Variants | workspace quick-create, session confirmation tray, expanded workspace delete row |
| States | normal, loading morph, contextual duplicate error, destructive confirm, cancel, success |
| Tokens | radius-8, color-surface-raised, color-border, border-thin, shadow-none, space-12, gap-6, z-overlay |
| Used in frame refs | B2 B3 B4 B5 B7 E7 G2 G4 G5 |
| Target framework | `dialog` · **adapt** · Proposed source-anchored quick-create/close tray and in-row delete. Source: `src/views/modals/mod.rs`. |

GPUI implementation constraint: Reuse modal state/events and stable ModalInput for B workspace quick-create, E7 close tray and G manager confirmation. C/D setup flows use compact-editor center-pane, not this dialog. Lock mutation while loading, retain errors and restore focus on cancel; source-attached geometry applies only to these B/E/G decisions.

### Anchored menu/popover

Quick-create uses the black/white canvas surface so its filled field remains distinct. Leave 12px between its heading and a 60px compound well: associated embedded Workspace label, UI-font value, and a flat trailing create action inside the well. Empty, loading, and duplicate states share this anatomy. Loading disables the input and action and exposes aria-busy; duplicate validation joins the full well width with a shared border and aria-describedby. Mini appbar specimens retain the same 24px controls as the appbar.

| Row | Content |
|---|---|
| Purpose | Places contextual actions and transient decisions directly against their source edge. |
| Anatomy | source anchor → deferred shared-edge surface → rows or stable quick-create form → optional in-place danger confirmation |
| Variants | workspace dropdown, project actions, workspace quick-create, manager delete expansion, session confirmation tray, tooltip |
| States | open, hover, keyboard focus, loading morph, contextual error, cancel, confirm, dismissed |
| Tokens | radius-8, color-surface-raised, color-border, shadow-none, z-overlay, space-8 |
| Used in frame refs | A2 A4 B1 D2 E2 G1 G6 G7 G8 |
| Target framework | `popover` · **add** · Proposed deferred(anchored()) surface with explicit dismissal lifecycle. Source: `src/views/appbar.rs`. |

## Workspace/project hierarchy

GPUI implementation constraint: Capture source bounds, attach shared edge and clamp to window; keep loading/error geometry stable. Raw anchored/deferred provides no focus or Escape logic. Vendored Popover appearance(false) also removes click-out dismissal.

### Disclosure

| Row | Content |
|---|---|
| Purpose | Expands or collapses project and worktree descendants. |
| Anatomy | icon button → right/down chevron → row label |
| Variants | project, worktree |
| States | collapsed, expanded, hover, focus-visible, disabled |
| Tokens | icon-12, control-h, color-text-muted, color-hover, color-focus |
| Used in frame refs | A1 A4 C4 D1 D6 E1 E4 F1 F4 F5 G3 |
| Target framework | `disclosure` · **adapt** · Adapted rows::render_row disclosure control. Source: `src/views/rows.rs`. |

GPUI implementation constraint: Dedicated right/down chevron control retains expanded state and accessible name. Keyboard activation must expand/collapse without accidentally selecting or launching; preserve selection while descendants hide.

### Project row

| Row | Content |
|---|---|
| Purpose | Represents a local Git project and its rollup actions. |
| Anatomy | 24px leading slot → flexible product-case project label and meta → fixed 48px status/count → fixed 28px trailing action |
| Variants | expanded, collapsed, non-Git error |
| States | loading, nothing yet, partial, error, hover, selected |
| Tokens | row-h, color-text-primary, color-text-muted, color-running, color-destructive, color-hover |
| Used in frame refs | A1 A4 C4 D1 D2 D6 E1 E4 F1 F4 F5 G3 |
| Target framework | `project-row` · **adapt** · Adapted rows::render_row(TreeRow::Project, RowCtx). Source: `src/views/rows.rs`. |

GPUI implementation constraint: 24px leading/flexible identity/48px status/28px action slots, stable at long names. Expose selection/disclosure and named project action; loading/non-Git/partial/error are explicit and never color-only.

### Worktree row

| Row | Content |
|---|---|
| Purpose | Represents one checkout and reveals direct-start agents on hover. |
| Anatomy | 24px leading slot → flexible product-case worktree label and branch meta → fixed 48px status/count → fixed 28px trailing action or direct-start reveal |
| Variants | main, branch, zero sessions, expanded, hovered |
| States | loading, nothing yet, partial, error, selected, hover |
| Tokens | row-h, radius-8, color-text-secondary, color-text-muted, color-running, color-hover, gap-4 |
| Used in frame refs | A1 A4 C4 D1 D6 E1 E2 E3 E4 F1 F4 F5 G3 |
| Target framework | `worktree-row` · **adapt** · Adapted rows::render_row(TreeRow::Worktree, RowCtx). Source: `src/views/rows.rs`. |

GPUI implementation constraint: Fixed slots and nonwrapping branch metadata; distinguish main/branch and zero sessions. Focus or hover reveals stable three-agent strip. Preserve SpawnAgent dispatch, and avoid clipping ring or shifting identity during reveal.

### Session row

| Row | Content |
|---|---|
| Purpose | Opens one session and states its process condition without relying on color. |
| Anatomy | 24px leading status slot → flexible agent/task label and context meta → fixed 48px status → fixed 28px trailing action |
| Variants | working, needs-you, done, idle, exited, starting |
| States | loading, nothing yet, partial, error, hover, selected, pending close |
| Tokens | row-h, color-selected, color-running, color-needs-you, color-destructive, color-text-muted, icon-14 |
| Used in frame refs | A1 A4 E3 E4 E5 E6 E7 F1 F2 F4 F5 G3 G7 |
| Target framework | `session-row` · **adapt** · Adapted rows::render_row(TreeRow::Session, RowCtx) + state_glyph. Source: `src/views/rows.rs`. |

## Sessions and terminal

GPUI implementation constraint: 24/flexible/48/28 slots; visible process label and context accompany glyph. Selection and close are distinct keyboard actions; pending close remains attached to source and preserves session until confirmation succeeds.

### Full-canvas active-session grid

| Row | Content |
|---|---|
| Purpose | Gives all active sessions the canvas while Grid is selected. |
| Anatomy | appbar with view switch → tile columns → resize seams → statusbar |
| Variants | one-column fallback, two-column fallback, three-column wide specimen |
| States | loading, nothing yet, partial, error, resizing |
| Tokens | color-bg, color-border, color-divider-soft, gap-2, duration-fast |
| Used in frame refs | F3 |
| Target framework | `grid` · **adapt** · Adapted grid::grid(GridCtx) below persistent appbar. Source: `src/views/grid.rs`. |

GPUI implementation constraint: Remove Sidebar in Grid, reuse column/row resize and session-focus dispatch. Provide one/two/three-column layouts and visible empty/loading/partial/error; clipping must not hide focused header controls.

### Active-session tile

| Row | Content |
|---|---|
| Purpose | Keeps one agent, task, workspace path, status, and clipped PTY together. |
| Anatomy | fixed agent icon → min-width-zero identity with separate task and path → nowrap status → clipped mono-only PTY |
| Variants | Codex working, Claude Code needs-you, Terminal running |
| States | Codex working, Claude Code needs-you, Terminal running, focused, error, loading, partial, resizing |
| Tokens | radius-8, color-surface, color-border, color-focus, color-accent, color-running, color-needs-you, color-terminal-bg |
| Used in frame refs | F3 |
| Target framework | `tile` · **adapt** · Adapted grid TileData composition with real terminal entity. Source: `src/views/grid.rs`. |

GPUI implementation constraint: Fixed agent glyph, min-width-zero identity/task, nowrap status, separate workspace/project/worktree path and clipped PTY. Preserve focus/resize behavior; attach close tray to tile and show state without color dependence.

### Workspace session list/card

| Row | Content |
|---|---|
| Purpose | Groups sessions in the selected workspace by Needs you, Working, and Idle. |
| Anatomy | status group header → session cards → retained selection |
| Variants | Needs you, Working, Idle groups |
| States | loading, nothing yet, partial, error, selected |
| Tokens | radius-8, color-surface, color-border, color-selected, color-needs-you, color-running, color-text-muted |
| Used in frame refs | F2 |
| Target framework | `session-list` · **adapt** · Adapted rows::flatten_sessions + TreeRow::SessionCard. Source: `src/views/rows.rs`. |

GPUI implementation constraint: Selected-workspace scope with Needs you/Working/Idle groups and retained selection. Cards use shrinking identity and fixed status/action slots; keyboard opens selected session, errors stay scoped and no filtering is added.

### PTY

| Row | Content |
|---|---|
| Purpose | Displays and accepts terminal process output for the selected session. |
| Anatomy | terminal surface → output lines → prompt/caret |
| Variants | agent PTY, Terminal PTY, clipped tile excerpt |
| States | loading, nothing yet, partial, error, running, awaiting input, exited |
| Tokens | color-terminal-bg, color-terminal-text, font-mono, text-12, color-running, color-needs-you, color-destructive |
| Used in frame refs | A1 A4 E3 E4 E5 E6 E7 F1 F2 F3 F4 F5 G3 G7 |
| Target framework | `pty` · **adapt** · Existing session terminal entity in session pane and TileData. Source: `src/views/grid.rs`. |

## Lists and rows

GPUI implementation constraint: Reuse existing terminal entity, IME, process lifecycle and resize dispatch; only surrounding geometry/theme changes. Clip output in tiles, keep UI labels sans and PTY monospace, and prevent overlay clicks from stealing input focus.

### Status group header

| Row | Content |
|---|---|
| Purpose | Labels a scoped session group and its count. |
| Anatomy | mono uppercase label → count chip → optional activity indicator |
| Variants | Needs you, Working, Idle, Terminals |
| States | loading, nothing yet, partial, error, expanded, collapsed |
| Tokens | row-h, font-mono, text-10, color-text-muted, color-needs-you, color-running, radius-full |
| Used in frame refs | F2 |
| Target framework | `group-header` · **adapt** · Adapted TreeRow::SectionHeader / rows::terminals_header. Source: `src/views/rows.rs`. |

## Inputs

Forms and inputs are first-class components. Creation uses center-pane editors or source-anchored popovers. The standard is a 60px flat filled well with a 12px regular embedded label above a 16px system-UI value, a 12px radius, and 12px between rows. Labels sit 14px from the left and 8px from the top; values start at 27px. Textareas reserve 130px. Regular form actions are 44px tall; toolbar controls remain 24px. Validation connects directly below its well and preserves the field value.

Wells use color-field-fill on the black or white canvas, a transparent default border, and a 1px neutral border plus 1px outline, totaling 2px. Muted icons remain subordinate to the value. Paths and branch names inside forms use the UI font; monospace remains for terminal output and compact metadata. Native labels identify each editable control; errors connect through aria-describedby and aria-invalid. Disabled controls retain legible values and cannot be edited; readonly controls remain focusable.

GPUI implementation constraint: Uppercase mono metadata, count and optional non-color activity cue. Match group spacing, expose expanded state only when interactive, and distinguish empty/loading/partial/error instead of inventing session filters.

### Text/path field

| Row | Content |
|---|---|
| Purpose | Captures workspace names and local Git paths without losing edits. |
| Anatomy | coherent 60px well → embedded regular label → UI-font value → attached help/error |
| Variants | text, path/branch, textarea, native select, embedded label, split/grouped fields |
| States | placeholder, focus-visible, filled, disabled, readonly, loading, invalid underline |
| Tokens | field-h, radius-12, font-ui, text-12, text-16, line-field-label, line-field-value, field-inset-x, field-inset-top, field-value-top, color-field-fill, color-border, color-focus, color-text-primary |
| Used in frame refs | B2 B3 B4 B5 B7 C2 C3 C5 D3 D4 D5 G2 G5 |
| Target framework | `field` · **adapt** · Stable ModalInput/InputState + proposed filled compound field shell. Source: `src/views/modals/input.rs`. |

GPUI implementation constraint: Existing field_box is paintless mono, not final. Build 60px radius12 filled well with 12px label at 14/8 and 16px UI-font value at y27; preserve entity/value, attach errors, explicitly override Input padding/type/line height and adapt native accessibility.

### Grouped switches

| Row | Content |
|---|---|
| Purpose | Exposes boolean preferences with explicit on/off state. |
| Anatomy | outlined group → label and help → switch track and white thumb |
| Variants | on, off, disabled |
| States | keyboard focus-visible, checked, unchecked, unavailable |
| Tokens | radius-16, switch-w, switch-h, switch-thumb, switch-inset, color-switch-on, color-switch-thumb, color-border, color-focus, opacity-disabled, duration-base |
| Used in frame refs | Component study; no new product flow is implied. |
| Target framework | `switches` · **add** · Proposed token-sized Grove switch and grouped rows. Source: `src/views/components.rs`. |

The HTML uses native buttons with role=switch and aria-checked. Space or Enter toggles the value; disabled switches cannot change. Text names the setting independently of color. Thumb travel is switch-w minus switch-thumb minus twice switch-inset.

GPUI implementation constraint: 48x28 track, 22px white thumb, 3px inset, 20px travel; off track uses COMPONENTS color-border. Role Switch, named setting/help, toggled state, focus-visible and Enter/Space; disabled cannot dispatch. Vendored Switch hardcodes different dimensions/motion.

### Selection tiles

| Row | Content |
|---|---|
| Purpose | Selects one or several clearly labeled values. |
| Anatomy | labeled group → compact filled tiles → neutral selected outline |
| Variants | single choice, multiple choices, disabled |
| States | unselected, selected, focus-visible, unavailable |
| Tokens | tile-min-w, tile-h, radius-12, text-14, gap-8, color-field-fill, color-selected, color-focus, border-thin, focus-ring |
| Used in frame refs | Agent and weekday component studies only. |
| Target framework | `choices` · **add** · Proposed Grove single/multiple selection tile group. Source: `src/views/components.rs`. |

Tiles use aria-pressed; a single-choice group always retains one selection, while multiple-choice tiles toggle independently. Keyboard focus has a separated neutral ring. State controls have no inserted action glyph. Weekday labels expose full accessible names.

GPUI implementation constraint: 42px minimum width, 44px height, radius12 and separated neutral focus ring. Named group and toggled buttons; single-choice retains one, multiple toggles independently, disabled stays inert. Weekday names are full accessible names; no action glyph.

### Date/time and category fields

| Row | Content |
|---|---|
| Purpose | Demonstrates native date/time editing and labeled category selection in the same well. |
| Anatomy | split labeled date and time wells; category dot alongside visible select value |
| Variants | date, time, native category select |
| States | editable, focused, selected |
| Tokens | field-h, field-inset-x, field-inset-top, field-value-top, radius-12, gap-12, category-dot-size, color-category-dot, color-field-fill |
| Used in frame refs | Component studies only; Grove scheduling is not proposed. |
| Target framework | `date-time` · **reference** · Reference-only date/time/category wells; no product integration. Source: `COMPONENTS.html`. |

Labels are associated with their inputs. The blue dot is decorative and never substitutes for the category name. Textareas use textarea-h; side-by-side wells collapse into one column when space is constrained.

GPUI implementation constraint: Component study only, no scheduling scope. Reuse well geometry and explicit labels if later requested; web native input/select cannot be copied into GPUI. Category dot is decorative, text names value; future native integration requires its own API verification.

### Inline validation

| Row | Content |
|---|---|
| Purpose | Explains a local field error while preserving all values. |
| Anatomy | field with aria-invalid → shared error border and attached message linked with aria-describedby → recovery action when needed |
| Variants | required, duplicate, non-Git, branch conflict, path conflict, backend failure |
| States | hidden, visible, updating, resolved |
| Tokens | text-12, color-destructive, color-error-wash, border-thin, radius-12, space-10, space-12 |
| Used in frame refs | B3 B7 C5 D4 E6 G4 G7 |
| Target framework | `validation` · **adapt** · Adapted note_text plus shared field/error shell and persistent error state. Source: `src/views/components.rs`. |

GPUI implementation constraint: Current red note alone is insufficient. Attach full-width message with shared border, preserve input entity/value and clear via owning edit lifecycle. Name error on editable accessibility node; pinned API has no aria_invalid/aria_describedby convenience methods.

### Inline-action input

| Row | Content |
|---|---|
| Purpose | Keeps a one-field action inside the input well without a footer. |
| Anatomy | optional leading semantic icon → flexible input → integrated trailing icon button |
| Variants | workspace create, generic submit |
| States | empty disabled, ready, loading in place, duplicate error |
| Tokens | field-h, gap-8, icon-14, radius-12, color-field-fill, color-destructive |
| Used in frame refs | B2-B5 B7 |
| Target framework | `inline-action` · **adapt** · Stable ModalInput + proposed embedded icon_btn action slot. Source: `src/views/components.rs`. |

GPUI implementation constraint: Keep 60px well, flexible value and fixed trailing action in one composition; empty disables submission, loading locks input/action without changing slots, duplicate adds attached error. IconButton was conceptual, not an existing Grove type.

### Browse-path input

| Row | Content |
|---|---|
| Purpose | Accepts a typed, dropped, suggested, or natively browsed repository folder. |
| Anatomy | embedded Repository label → folder icon aligned with UI-font value → trailing Browse icon button → directory matches |
| Variants | empty, typed, suggestions, canonical readonly |
| States | focus-visible, disabled, probing, invalid folder |
| Tokens | field-h, gap-8, icon-14, radius-12, color-field-fill, color-focus, color-destructive |
| Used in frame refs | C2 C3 C6 C7 |
| Target framework | `browse-path` · **adapt** · ModalInput + native browse/drop/suggestions through add_project::choose. Source: `src/views/modals/add_project.rs`. |

GPUI implementation constraint: Embedded Repository label, muted folder and UI-font value with named Browse action. Typed, selected, dropped and browsed paths converge through choose/choose_typed for canonicalization and probe; readonly remains focusable and errors preserve draft.

### Repository probe row

| Row | Content |
|---|---|
| Purpose | Reports canonical repository status before registration. |
| Anatomy | state icon → result → current branch or Initialize Git choice |
| Variants | Git repository with branch, Not a Git repository with Initialize Git enabled by default |
| States | probing, success, not Git, local error |
| Tokens | row-h, gap-8, icon-14, color-running, color-needs-you, color-text-secondary |
| Used in frame refs | C3 C6 |
| Target framework | `repo-probe` · **adapt** · Existing is_repo/init_git state in compact probe row. Source: `src/add_project.rs`. |

GPUI implementation constraint: State icon, repository result and branch or Initialize Git default-on choice. Preserve probing/success/not-Git/error distinctions; switch semantics and named help are required, and probe changes must not discard typed path.

### Compact form/editor

| Row | Content |
|---|---|
| Purpose | Replaces the center pane for project and worktree setup without a modal card. |
| Anatomy | compact title → ghost cancel → embedded-label rows and compound wells → probe/dividers → 44px submit |
| Variants | Add project PickSource, Add project Details, New worktree |
| States | initial, probing, ready, loading locked, attached field errors |
| Tokens | header-h, field-h, form-button-h, radius-12, gap-12, border-thin, color-bg, color-field-fill, color-divider-soft |
| Used in frame refs | C2-C4 C6-C7 D3-D5 |
| Target framework | `compact-editor` · **adapt** · Proposed center-pane editors using existing modal state/dispatch. Source: `src/views/modals/add_project.rs`. |

## Progress and feedback

GPUI implementation constraint: Rehost AddProject and NewWorktree behavior in center pane, with compact title/Cancel, 60px compound wells, 12px row gaps and 44px submit. Preserve focus policy/native picker/errors; lock loading in place rather than replace form with modal.

### Empty state

| Row | Content |
|---|---|
| Purpose | Explains what the selected workspace lacks and offers its next valid action. |
| Anatomy | title → scoped explanation → optional primary setup action with mapped leading SVG |
| Variants | no projects, no sessions, no active Grid sessions |
| States | loading handoff, nothing yet, partial, error |
| Tokens | text-24, text-13, color-text-primary, color-text-secondary, space-24, gap-8 |
| Used in frame refs | A5 B6 C1 C4 D6 E1 F3 |
| Target framework | `empty` · **adapt** · Adapted grid::empty_state / TreeRow::Empty. Source: `src/views/grid.rs`. |

GPUI implementation constraint: Scoped title/explanation and valid setup action with leading icon. No New session outside worktree launch. Keyboard-operable setup action only when available; distinguish empty from loading/error/partial and retain navigation.

### Progress/skeleton

| Row | Content |
|---|---|
| Purpose | Holds layout steady while switching or creating local resources. |
| Anatomy | reserved row or panel → neutral bars → concise progress label |
| Variants | sidebar skeleton, content progress, locked dialog progress |
| States | loading, partial, success handoff, error |
| Tokens | radius-8, color-surface, color-selected, color-text-muted, duration-slow, opacity-muted |
| Used in frame refs | A3 B5 C3 D5 E3 |
| Target framework | `progress` · **add** · Proposed token-only progress/skeleton in owning view. Source: `src/views/components.rs`. |

GPUI implementation constraint: Reserve final row/form slots, use concise visible loading text and prevent repeated dispatch. Preserve focus/input entity through loading, show scoped failure with recovery, and reduce motion to static feedback without an animation timer.

### Status chip/banner

| Row | Content |
|---|---|
| Purpose | Labels status or explains feedback with words and a non-color cue. |
| Anatomy | optional non-button status glyph → label → optional icon-led action button |
| Variants | selected, waiting, running/success, warning, error, info |
| States | loading, nothing yet, partial, error, dismissed |
| Tokens | radius-full for chips, radius-8 for banners, color-running, color-needs-you, color-destructive, color-tertiary, color-text-primary |
| Used in frame refs | A2 A4 B3 B7 C5 D4 E3 E4 E5 E6 G4 G6 G7 |
| Target framework | `status` · **adapt** · Adapted status_dot/status_dot_hollow/keycap_filled and banners. Source: `src/views/components.rs`. |

GPUI implementation constraint: Pills use radius-full, banners radius8; status words and non-color glyph identify condition. Actions have semantic icon/name/keyboard focus. Existing keycap geometry is not a final pill; errors preserve context and offer only supported recovery.


## GPUI component implementation map

The HTML `gpuiComponentMap` literal mirrors these exact IDs, targets and statuses. **verified** means an existing implementation matches; **adapt** means an existing behavior/seam needs geometry, state or semantic work; **add** means the composition/helper is missing; **reference** means component study only. No item is marked verified merely because a similarly named helper exists. Existing source paths are navigation anchors, not claims that proposed symbols already exist.

| ID | Target composition | Status | Source | Implementation contract |
|---|---|---|---|---|
| button | ModalBtn + adapted modal_action/modal_action_sized/body_action | adapt | src/views/components.rs | Keep semantic icon and progress in one reserved leading slot; regular actions 44px, compact 24px. Existing text-only mouse-down helpers need focus-visible, keyboard/on_click, disabled dispatch guards and stable loading geometry. |
| icon | Adapted icon_btn/flat_icon_btn + hint_tooltip | adapt | src/views/components.rs | Fixed 24px action box, semantic SVG, accessible name and matching tooltip. Add role, focus and keyboard activation; retain nested-row propagation protection. Tooltip alone does not name the control. |
| trio | Worktree-local Codex/Claude Code/Terminal actions via RowAction::SpawnAgent | adapt | src/views/rows.rs | Reveal three fixed launch slots on hover OR keyboard focus without shifting labels; name each agent and worktree. Current available-agent loop also exposes Run/More/Delete; unavailable/starting/error states need explicit treatment. |
| switch | Destination toggle + Grid, placed by screens composition | add | src/views/appbar.rs | Two separate borderless 24px buttons with 2px gap, labels/tooltips and Grid toggled state. screens.html places them in sidebar side-head for Project/List and appbar for Grid, never both. Preserve selection and remembered non-grid destination; joined seg_group is not the target. |
| appbar | Adapted appbar::appbar with workspace trigger and Grid-mode view controls | adapt | src/views/appbar.rs | Stable full-width brand/workspace/tools bar. Follow screens.html placement: view controls live here only in Grid, and in sidebar side-head for Project/List. COMPONENTS appbar specimen differs; preserve its styling, not duplicate controls. Workspace loading/retry must not move unrelated navigation. |
| trigger | Proposed workspace trigger inside appbar::appbar | add | src/views/appbar.rs | 24px leading slot, min-width-zero flexible label/meta, 28px chevron slot. Named keyboard-operable trigger exposes expanded state; keep source bounds and focus handle for dismissal return. |
| workspace-menu | Proposed anchored workspace dropdown with pinned actions | add | src/views/appbar.rs | Title and Create/Manage are siblings of a constrained scrolling row region. Rows use 24/flexible/48/28 slots, named selected state, keyboard navigation and visible loading/empty/partial/error; Escape/outside click restores focus. |
| sidebar | Adapted Sidebar::render + rows::render_row | adapt | src/views/sidebar.rs | Project and List retain 236px hierarchy sidebar with two view controls in side-head per screens.html; Grid omits sidebar and places controls in appbar. Preserve scroll/selection and collapsed semantics; reveal worktree launch on keyboard focus. No duplicate view controls or session filtering. |
| session-header | Adapted session_header::session_header(SessionHeaderData, ToolCluster) | adapt | src/views/session_header.rs | Session identity and context precede diff and named icon tools. Reuse dispatch; apply header geometry and words/non-color cues for loading, running, needs-you, failed and branchless terminal. |
| statusbar | Adapted statusbar::statusbar(StatusbarCtx) | adapt | src/views/statusbar.rs | Running group, backend/theme, existing toast slot, spacer, hints/version. Apply tokens and explicit status words; actionable retries need named keyboard-operable controls, while passive messages do not steal focus. |
| scrim | Proposed neutral veil + source repaint + anchored tray in ModalLayer | adapt | src/views/modals/mod.rs | Current scrim centers content and is not final presentation. Paint low-opacity veil only, then crisp initiating row/tile and edge tray; block underlying PTY mouse/keyboard interaction, trap decision focus and restore it on Cancel/Escape. |
| dialog | Proposed source-anchored quick-create/close tray and in-row delete | adapt | src/views/modals/mod.rs | Reuse modal state/events and stable ModalInput for B workspace quick-create, E7 close tray and G manager confirmation. C/D setup flows use compact-editor center-pane, not this dialog. Lock mutation while loading, retain errors and restore focus on cancel; source-attached geometry applies only to these B/E/G decisions. |
| popover | Proposed deferred(anchored()) surface with explicit dismissal lifecycle | add | src/views/appbar.rs | Capture source bounds, attach shared edge and clamp to window; keep loading/error geometry stable. Raw anchored/deferred provides no focus or Escape logic. Vendored Popover appearance(false) also removes click-out dismissal. |
| disclosure | Adapted rows::render_row disclosure control | adapt | src/views/rows.rs | Dedicated right/down chevron control retains expanded state and accessible name. Keyboard activation must expand/collapse without accidentally selecting or launching; preserve selection while descendants hide. |
| project-row | Adapted rows::render_row(TreeRow::Project, RowCtx) | adapt | src/views/rows.rs | 24px leading/flexible identity/48px status/28px action slots, stable at long names. Expose selection/disclosure and named project action; loading/non-Git/partial/error are explicit and never color-only. |
| worktree-row | Adapted rows::render_row(TreeRow::Worktree, RowCtx) | adapt | src/views/rows.rs | Fixed slots and nonwrapping branch metadata; distinguish main/branch and zero sessions. Focus or hover reveals stable three-agent strip. Preserve SpawnAgent dispatch, and avoid clipping ring or shifting identity during reveal. |
| session-row | Adapted rows::render_row(TreeRow::Session, RowCtx) + state_glyph | adapt | src/views/rows.rs | 24/flexible/48/28 slots; visible process label and context accompany glyph. Selection and close are distinct keyboard actions; pending close remains attached to source and preserves session until confirmation succeeds. |
| grid | Adapted grid::grid(GridCtx) below persistent appbar | adapt | src/views/grid.rs | Remove Sidebar in Grid, reuse column/row resize and session-focus dispatch. Provide one/two/three-column layouts and visible empty/loading/partial/error; clipping must not hide focused header controls. |
| tile | Adapted grid TileData composition with real terminal entity | adapt | src/views/grid.rs | Fixed agent glyph, min-width-zero identity/task, nowrap status, separate workspace/project/worktree path and clipped PTY. Preserve focus/resize behavior; attach close tray to tile and show state without color dependence. |
| session-list | Adapted rows::flatten_sessions + TreeRow::SessionCard | adapt | src/views/rows.rs | Selected-workspace scope with Needs you/Working/Idle groups and retained selection. Cards use shrinking identity and fixed status/action slots; keyboard opens selected session, errors stay scoped and no filtering is added. |
| pty | Existing session terminal entity in session pane and TileData | adapt | src/views/grid.rs | Reuse existing terminal entity, IME, process lifecycle and resize dispatch; only surrounding geometry/theme changes. Clip output in tiles, keep UI labels sans and PTY monospace, and prevent overlay clicks from stealing input focus. |
| group-header | Adapted TreeRow::SectionHeader / rows::terminals_header | adapt | src/views/rows.rs | Uppercase mono metadata, count and optional non-color activity cue. Match group spacing, expose expanded state only when interactive, and distinguish empty/loading/partial/error instead of inventing session filters. |
| field | Stable ModalInput/InputState + proposed filled compound field shell | adapt | src/views/modals/input.rs | Existing field_box is paintless mono, not final. Build 60px radius12 filled well with 12px label at 14/8 and 16px UI-font value at y27; preserve entity/value, attach errors, explicitly override Input padding/type/line height and adapt native accessibility. |
| switches | Proposed token-sized Grove switch and grouped rows | add | src/views/components.rs | 48x28 track, 22px white thumb, 3px inset, 20px travel; off track uses COMPONENTS color-border. Role Switch, named setting/help, toggled state, focus-visible and Enter/Space; disabled cannot dispatch. Vendored Switch hardcodes different dimensions/motion. |
| choices | Proposed Grove single/multiple selection tile group | add | src/views/components.rs | 42px minimum width, 44px height, radius12 and separated neutral focus ring. Named group and toggled buttons; single-choice retains one, multiple toggles independently, disabled stays inert. Weekday names are full accessible names; no action glyph. |
| date-time | Reference-only date/time/category wells; no product integration | reference | COMPONENTS.html | Component study only, no scheduling scope. Reuse well geometry and explicit labels if later requested; web native input/select cannot be copied into GPUI. Category dot is decorative, text names value; future native integration requires its own API verification. |
| inline-action | Stable ModalInput + proposed embedded icon_btn action slot | adapt | src/views/components.rs | Keep 60px well, flexible value and fixed trailing action in one composition; empty disables submission, loading locks input/action without changing slots, duplicate adds attached error. IconButton was conceptual, not an existing Grove type. |
| browse-path | ModalInput + native browse/drop/suggestions through add_project::choose | adapt | src/views/modals/add_project.rs | Embedded Repository label, muted folder and UI-font value with named Browse action. Typed, selected, dropped and browsed paths converge through choose/choose_typed for canonicalization and probe; readonly remains focusable and errors preserve draft. |
| validation | Adapted note_text plus shared field/error shell and persistent error state | adapt | src/views/components.rs | Current red note alone is insufficient. Attach full-width message with shared border, preserve input entity/value and clear via owning edit lifecycle. Name error on editable accessibility node; pinned API has no aria_invalid/aria_describedby convenience methods. |
| repo-probe | Existing is_repo/init_git state in compact probe row | adapt | src/add_project.rs | State icon, repository result and branch or Initialize Git default-on choice. Preserve probing/success/not-Git/error distinctions; switch semantics and named help are required, and probe changes must not discard typed path. |
| compact-editor | Proposed center-pane editors using existing modal state/dispatch | adapt | src/views/modals/add_project.rs | Rehost AddProject and NewWorktree behavior in center pane, with compact title/Cancel, 60px compound wells, 12px row gaps and 44px submit. Preserve focus policy/native picker/errors; lock loading in place rather than replace form with modal. |
| empty | Adapted grid::empty_state / TreeRow::Empty | adapt | src/views/grid.rs | Scoped title/explanation and valid setup action with leading icon. No New session outside worktree launch. Keyboard-operable setup action only when available; distinguish empty from loading/error/partial and retain navigation. |
| progress | Proposed token-only progress/skeleton in owning view | add | src/views/components.rs | Reserve final row/form slots, use concise visible loading text and prevent repeated dispatch. Preserve focus/input entity through loading, show scoped failure with recovery, and reduce motion to static feedback without an animation timer. |
| status | Adapted status_dot/status_dot_hollow/keycap_filled and banners | adapt | src/views/components.rs | Pills use radius-full, banners radius8; status words and non-color glyph identify condition. Actions have semantic icon/name/keyboard focus. Existing keycap geometry is not a final pill; errors preserve context and offer only supported recovery. |

## GPUI implementation recipes

These are **schematic implementation recipes, not compiled samples or completed helpers**. Uppercase finalized token names and semantic theme functions below denote the target mappings in DESIGN; resolve/add them before implementation. Preserve the existing input entities, state machines and dispatch. Use the `html-to-gpui` project skill for the full handoff and validation workflow.

### Pinned API evidence

`Cargo.toml` pins GPUI to `1a246efd7e1b83ab568ec5e3e6c1a43a42e1abba`. The local source is under `~/.cargo/git/checkouts/zed-a70e2ad075855582/1a246ef/crates/gpui/src/`; treat that absolute cache path as a local locator, not a portable project dependency. Verify the matching revision when locating it on another machine. Vendored gpui-component is `88f102d13654fe25aa2fede076274b6b751a3704`, with provenance/patch history in `vendor/gpui-component/README.md`.

| Concern | Verified local source | Usable API / limit |
|---|---|---|
| Stateful semantics | GPUI `elements/div.rs`, `StatefulInteractiveElement` | `role`, `aria_label`, `aria_description`, `aria_selected`, `aria_expanded`, `aria_toggled`, `aria_value`, `aria_placeholder`, `on_a11y_action`, `focusable`, `focus_visible`, `on_click`. GPUI reexports `Role`, `Toggled`, `AccessibleAction` and `accesskit`. |
| Focus and events | GPUI `elements/div.rs` | `track_focus`, `tab_stop`, `tab_index`, `occlude`; focusable `on_click` already handles Enter/Space and accessibility activation. Do not dispatch again in a second key handler. |
| Input | `vendor/gpui-component/ui/src/input/input.rs` | `appearance`, `disabled`, `role`, `tab_index`, `prefix`, `suffix`, `bordered`, `focus_bordered`, `Styled`. No input label/description/readonly setter exists. |
| Input state | `src/views/modals/input.rs`; vendor `input/state.rs` | Stable `Entity<InputState>`, `set_value`, `focus`, `set_selected_range`; selection ranges use UTF-8 **bytes**. `ModalInput::focus_at_end` uses byte length correctly. |
| Scroll | `src/views/sidebar.rs`; GPUI `elements/div.rs` | Stable `ScrollHandle`, `overflow_y_scroll`, `track_scroll`, `scroll_to_item`; constrain parent height and use `min_h_0`. |
| Anchors | GPUI `elements/anchored.rs`, `elements/deferred.rs` | `anchor`, `position`, `offset`, `position_mode`, `snap_to_window`, `snap_to_window_with_margin`; `deferred(...).with_priority(...)`. No automatic dismissal or focus ownership. |
| Popover wrapper | `vendor/gpui-component/ui/src/popover.rs` | `track_focus`, `on_open_change`, `overlay_closable`; `appearance(false)` removes background/border/shadow/padding **and click-out dismissal**. |
| Switch | `vendor/gpui-component/ui/src/switch.rs` | Existing 36×20/16px thumb or 28×16/12px thumb, 2px inset and 150ms animation are not final 48/28/22/3 and 140ms. `with_size` cannot express final geometry. No built-in role/focus/key wiring. |

The pinned Div convenience API has no `aria_invalid`, `aria_busy`, `aria_disabled`, `aria_describedby` or `aria_labelledby` setters. Do not paste browser attributes as invented Rust methods. Use supported names/descriptions/state where available, and document/test any required AccessKit or tracked vendor extension. A tooltip or named outer group is not proof that the actual editable accessibility node has a label or error description. Input readonly must prevent editing while preserving focus/selection/copy; `disabled(true)` is not its equivalent. Honor vendor provenance and record minimal extensions instead of silently upgrading GPUI.

### Compound well and stable input

Known workspace-create reference discrepancy: COMPONENTS defines the final embedded inline-action well and pending state with disabled Close plus busy indication. screens.html createWorkspaceDialog still shows footer actions and an enabled Close while pending. Use COMPONENTS for that component anatomy/state; synchronize the B-frame specimen before accepting it as a native screenshot baseline. This handoff preserves both reference visuals.

Own `ModalInput`/`Entity<InputState>` in the view, not in its render function. Render a filled 60px well on the canvas, embedded 12px regular label at x14/y8 with 16px line height, and a 16px UI-font value at y27 with 22px line height. Preserve 12px radius, 12px inter-field gap, and 130px textarea geometry. The conceptual tree is:

```text
field group (stable id, visible label/help/error)
  60px relative filled well (reserved transparent 1px border)
    label positioned x14/y8, 12px UI / 16px line
    value row positioned x14/y27, 22px high, right inset14
      optional fixed muted icon
      shrinking Input tied to existing InputState
      optional fixed trailing icon_btn
  attached full-width error zone with shared border
```

The value-row recipe uses real APIs with pending token names:

```rust,ignore
// Parent value row has a definite 22px token height and flexible width.
Input::new(modal_input.state())
    .appearance(false)
    .pl(px(0.0))
    .pr(px(0.0))
    .py(px(0.0))
    .size_full()
    .font(gpui::font(UI_FAMILY))
    .text_size(rpx(TEXT_16))
    .line_height(rpx(LINE_FIELD_VALUE))
    .disabled(loading)
```

`appearance(false)` only removes paint; it does not remove default insets, text sizing or line height. `Input::render` defaults to `Rems(1.25)` line height and then refines from explicit style, so override all three. Its inherent `.h(...)` and `.h_full()` affect the **multiline** branch only; use definite parent value-row geometry and explicit Styled sizing (`size_full` above) for single-line content. Verify computed size rather than assuming `.h(60px)` controls it. Prefix/suffix code reintroduces padding after style refinement; keep compound icons/actions as external sibling slots when exact insets matter.

Do not claim this visual shell completes native semantics. A minimal documented vendor adaptation must name/describe the real input node, represent error/busy state as supported, and implement actual readonly policy. Preserve IME, clipboard and selection behavior. Disabled dispatch guards belong to action/state ownership as well as input styling. Error/loading morphs must not recreate the entity, discard edits, steal focus, or collapse the action slot.

### Button, switch and selection semantics

Use stable per-instance IDs and the appropriate role. This schematic shows the shared activation seam; the final tokenized paint and separated focus-ring helper still need implementation:

```rust,ignore
// id, label, checked, enabled and dispatch are supplied by the owning view.
div()
    .id(id)
    .role(gpui::Role::Switch)
    .aria_label(label)
    .aria_toggled(if checked { gpui::Toggled::True } else { gpui::Toggled::False })
    .when(enabled, |control| control.focusable().on_click(move |_, window, cx| {
        cx.stop_propagation();
        dispatch(!checked, window, cx);
    }))
    // Add 48x28 track /22 thumb /3 inset, final tokens and focus-visible paint.
```

Use `gpui::Role::Button` for actions and selection tiles, adding `aria_toggled` for pressed tile state; this pinned AccessKit has no `Role::ToggleButton`. Name icon-only actions and attach matching tooltips. `.focusable().on_click(...)` supports Enter/Space directly in this pinned GPUI. Existing helpers use `.on_mouse_down(...)`; adding another click handler without removing/adapting the old action risks duplicate activation. Nested row activation must still be stopped. A disabled or loading control must omit/guard dispatch; opacity alone is not inertness. Keep state visible and legible, including disabled text and icons.

Focus rings must be neutral, separated where specified, visible beyond the control and not clipped by a parent's `overflow_hidden`. Reserving a transparent border avoids layout movement, but a border-only selected state is not the full separated keyboard-focus ring. The field has a 1px neutral border plus 1px outside outline; implement without shrinking the text box or moving content. Use `focus_visible` for keyboard indication and the tracked input focus for the well; do not invent a CSS `focus-within` method. GPUI `in_focus` styles an element inside a focused ancestor; it is not a parent-side CSS `:focus-within` replacement. Derive the well state from its retained input focus handle and verify focus traversal for the actual tree. Scope opacity to the disabled control or scrim layer; applying scrim opacity to a parent would fade the source repaint and tray too.

For switches, `48 - 22 - 2*3 = 20px` travel. Final base duration is 140ms, not vendor 150ms. Vendor `cubic_bezier` is an approximation that returns y(t) without solving x(t); do not claim it exactly reproduces CSS easing. Follow DESIGN’s planned easing mapping for accurate motion. Reduced motion should paint endpoint state directly instead of scheduling a zero-duration animation. Off-track discrepancy is recorded: DESIGN's specimen uses `color-border-strong`, COMPONENTS uses `color-border`. **COMPONENTS owns component state anatomy, so use `color-border` for this switch**; token values still come from DESIGN. This handoff does not alter either specimen's visuals. Selection tiles use no action icons; single-choice never toggles the selected value off and multi-choice toggles independently.

### Fixed-slot rows, scrolling and overlays

Screen composition owns control placement: `screens.html` renders the two Project/List/Grid controls in `.side-head` for Project/List, and in the appbar for Grid. Render one set only. The standalone COMPONENTS appbar specimen shows view controls there across modes; preserve that specimen’s component styling while following screens for actual placement.

For rows, compose `24px leading / flex_1 + min_w_0 label/meta / 48px status / 28px trailing`. Fixed slots use `flex_shrink_0`, flexible labels truncate explicitly, and metadata must not accidentally wrap inside a fixed row height. The three launch controls require a reserved reveal region; do not squeeze three controls into the 28px single-action slot. Status text must survive narrow widths; tile task and workspace path are separate lines. Scroll only the bounded rows (`id`, `overflow_y_scroll`, stable `track_scroll`); title and pinned footer/actions are nonshrinking siblings. Avoid clipping rings while clipping text and PTY output.

Source-attached overlay recipe, with source bounds/focus/lifecycle supplied by the owner:

```rust,ignore
// source_edge is a captured point in WINDOW coordinates, not design-board coordinates.
gpui::deferred(
    gpui::anchored()
        .anchor(gpui::Anchor::TopLeft)
        .position(source_edge)
        .snap_to_window_with_margin(rpx(SPACE_LG).to_pixels(window.rem_size()))
        .child(tray), // zero external margin; tokenized shared-edge surface
)
.with_priority(overlay_priority)
```

`snap_to_window_with_margin` requires `Into<Edges<Pixels>>`, not `Rems`; convert the existing 8px `SPACE_LG` token through `rpx(SPACE_LG).to_pixels(window.rem_size())`. `Pixels` converts into uniform edges (GPUI `geometry.rs`); this is the coordinate-type boundary. `anchored` defaults to window coordinates; do not apply `rpx` again to measured bounds. Its anchor denotes the tray's own corner. It defaults to switching anchor on overflow; choose snap behavior deliberately and verify edge attachment near all viewport sides. Priority orders deferred painting, not arbitrary CSS z-index. Capture initiating bounds during layout/prepaint and keep them current through resizing and scrolling. The owner saves prior focus, focuses the field/first action on open, contains keyboard interaction, handles Escape/Cancel/outside click and restores the initiating control if still present (otherwise a defined valid fallback). Loading blocks duplicate mutation. A delete confirmation expands its actual manager row instead of opening a centered dialog.

For modal decisions, paint normal content, then an input-blocking quiet veil, then the crisp initiating source and tray. Keep the underlying app inert; `ModalLayer` already uses `.occlude()` to stop terminal mouse-down focus theft. Raw `anchored` does not provide that protection, keyboard trapping or dismiss handling. Existing `scrim` and `panel_surface` paint centered layouts/shadows and must not be treated as final equivalents. If adapting vendor Popover, explicitly restore click-out handling when `appearance(false)` is used.

### Review evidence for an implementation

Before calling a native component accurate, compare the matching specimen and frames at 100% app zoom in dark/light, then supported zoom and narrow widths. Inspect regular/compact geometry, long labels/paths, fixed slots, focus rings, keyboard-only reveal/activation, input selection/IME, disabled/readonly/loading/error morphs and source-attached dismiss/focus return. Verify native semantics on the actual editable/action node. Preserve existing state-machine and PTY behavior checks; HTML syntax or a screenshot alone does not prove the Rust overhaul works. Follow the project skill for the implementation's appropriate native build/test and screenshot evidence. This document preparation runs no native build and adds no product behavior.
