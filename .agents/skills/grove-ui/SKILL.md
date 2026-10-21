---
name: grove-ui
description: Use for Grove native UI work from a screenshot, HTML mock, design spec, or visual critique, and for changes to sidebar, session/grid views, forms, menus, or modals where layout and interaction states must match the current design.
---

# Grove UI implementation

Grove is a Rust/GPUI app. Read [DESIGN.md](../../../DESIGN.md) for the approved target, then inspect the current view and its shared primitives before changing it. `DESIGN.md` includes pending migration targets and older proposals: use the latest explicit user direction when they differ, and verify the current implementation instead of copying a mock's table or numbers literally.

## Establish the implementation contract

Before UI implementation or delegation, record a short contract in the existing plan or task state: the latest approved reference or direction, placement, included content and exclusions, behavior and visibility conditions, and acceptance checks. Settled decisions need no reconfirmation or separate document. When direction changes, reconcile the contract with completed and unfinished work and propagate the updated contract to every worker brief before dependent work continues.

## Build from the existing system

- Work in `src/views/`; use `src/views/tokens.rs` for dimensions, `src/theme.rs` for semantic colors, `src/views/components.rs` for shared controls, and `src/icons.rs` for icons. Add a shared role when a genuine repeated need is missing; avoid one-off visual variants that drift between screens.
- Use one app-level System/Dark/Light appearance setting with fixed palettes. Workspace and project views do not choose colors.
- Treat a screenshot or HTML mock as evidence of hierarchy, spacing, and behavior. Map it to GPUI structure and current user decisions; do not port CSS or stale framework code mechanically.

## Preserve visual discipline

- Set clear UI type roles for headings, labels, form values, metadata, status, and supporting copy. Form values and metadata, including paths and branches, use UI sans. Reserve monospace for terminal/code content unless the user approves another role. Check line height, truncation, and contrast at actual display scale.
- Align sidebar icons, titles, metadata, counts, and trailing actions to stable columns. Keep section labels and nested rows on the intended inset. Size headers to their remaining content after controls move.
- Give sidebar rows, main content, terminals, forms, and dialogs deliberate internal padding. Related inputs share one field well, label/value spacing, and border treatment.
- Keep grid tiles flush, with subtle seams including the top edge. Avoid item gaps and heavy tile borders.
- Distinguish hover, keyboard focus (`focus_visible`), selected rows, open menus, and invalid fields. Action buttons return to their resting appearance after release; they do not retain a clicked highlight. Preserve intentional selected/open fill and visible keyboard focus. Form focus uses a neutral fill and stable border; validation can use its semantic border.

## Verify the result

Compare the actual native rendering with the contract's selected reference (or approved existing design) for spacing, typography, icons, clipping, alignment, contrast, and affected interaction states. Record the exact build and worktree state inspected, screenshot or interaction evidence, and which acceptance checks passed, failed, or were unobserved. Correct observed drift before calling the UI done. A worker's success claim does not replace the orchestrator's own observed evidence. If native inspection is blocked, describe the blocker and unobserved checks; do not claim visual verification.

Choose interaction checks from the changed behavior and regression risk, rather than inspecting every view and state on every change. For example:

- For flexible rows or panels, inspect long names at desktop and narrower widths for clipping, spacing, and trailing-action alignment.
- For expanding tasks or disclosures, expand and collapse while checking surrounding geometry and minimap position.
- For keyboard routing or embedded HTML, type into the affected input, including spaces, and exercise relevant shortcuts.
- For session-scoped state, switch sessions and return; for compaction or disclosure changes, compact and check that intended disclosure state survives.
- For changed controls, inspect applicable resting, hover, keyboard-focus, selected/open, validation, and release states; include affected list/grid, terminal, or dialog contexts only where relevant.

Follow [AGENTS.md](../../../AGENTS.md) for delivery and installation rules and, for shared agent features, Codex/Claude acceptance coverage. Native inspection can use the current build in an isolated QA app; identify the observed build and leave unobserved behavior explicit when installation is deferred or prohibited.
