---
name: grove-ui
description: Use for Grove native UI work from a screenshot, HTML mock, design spec, or visual critique, and for changes to sidebar, session/grid views, forms, menus, or modals where layout and interaction states must match the current design.
---

# Grove UI implementation

Grove is a Rust/GPUI app. Read [DESIGN.md](../../../DESIGN.md) for the approved target, then inspect the current view and its shared primitives before changing it. `DESIGN.md` includes pending migration targets and older proposals: use the latest explicit user direction when they differ, and verify the current implementation instead of copying a mock's table or numbers literally.

## Build from the existing system

- Work in `src/views/`; use `src/views/tokens.rs` for dimensions, `src/theme.rs` for semantic colors, `src/views/components.rs` for shared controls, and `src/icons.rs` for icons. Add a shared role when a genuine repeated need is missing; avoid one-off visual variants that drift between screens.
- Use one app-level light/dark theme across the UI. Workspace and project views do not choose their own themes.
- Treat a screenshot or HTML mock as evidence of hierarchy, spacing, and behavior. Map it to GPUI structure and current user decisions; do not port CSS or stale framework code mechanically.

## Preserve visual discipline

- Set clear UI type roles for headings, labels, form values, metadata, status, and supporting copy. Form values and metadata, including paths and branches, use UI sans. Reserve monospace for terminal/code content unless the user approves another role. Check line height, truncation, and contrast at actual display scale.
- Align sidebar icons, titles, metadata, counts, and trailing actions to stable columns. Keep section labels and nested rows on the intended inset. Size headers to their remaining content after controls move.
- Give sidebar rows, main content, terminals, forms, and dialogs deliberate internal padding. Related inputs share one field well, label/value spacing, and border treatment.
- Keep grid tiles flush, with subtle seams including the top edge. Avoid item gaps and heavy tile borders.
- Distinguish hover, keyboard focus (`focus_visible`), selected rows, open menus, and invalid fields. Action buttons return to their resting appearance after release; they do not retain a clicked highlight. Preserve intentional selected/open fill and visible keyboard focus. Form focus uses a neutral fill and stable border; validation can use its semantic border.

## Verify the result

Inspect the installed native app after a UI change, alongside the reference when one exists. Check desktop and narrower widths where the view can appear; normal, hover, open, and keyboard states; list and grid; terminal content; and affected dialogs. Look specifically for clipping, unbalanced gaps, weak contrast, misaligned baselines, excessive header height, and inconsistent control states. Correct observed drift before calling the UI done. Use the relevant build/tests and the project's install workflow in `CLAUDE.md`.
