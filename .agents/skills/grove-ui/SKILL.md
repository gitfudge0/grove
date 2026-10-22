---
name: grove-ui
description: Implement and review Grove native UI, including navigation, forms, menus, settings, transcript/composer, task docks, native plans, and HTML reports, against the shared design contract and affected interaction states.
---

# Grove UI implementation

Read [DESIGN.md](../../../DESIGN.md) before UI implementation or delegation. It owns approved hierarchy, semantics, interaction states, and exceptions; current tokens/theme/components own exact values. Historical `DESIGN.html` and archived migration tables are provenance, not implementation specifications. Explicit current user direction takes precedence; reconcile it with the contract rather than silently changing product direction.

## Establish the contract

Record the approved reference/direction, placement, content/exclusions, behavior/visibility, and acceptance checks in the existing task state. Settled decisions need no reconfirmation or separate artifact. Reconcile changed direction with completed and pending work, and propagate it to every worker before dependent work continues. Preserve the selected delivery endpoint and any no-install/stop instruction.

Inspect the owning native view plus [tokens](../../../src/views/tokens.rs), [theme](../../../src/theme.rs), [components](../../../src/views/components.rs), and [icons](../../../src/icons.rs). Match a screenshot/mock to these primitives; do not copy CSS values or invent GPUI APIs. Use the contract’s control decision table: complete dense/form action recipes, form/project field wells, question/settings controls, attached validation, selectable readonly values, and shared switches/tooltips/status marks. Do not fork their shape locally. Native vendor inputs inside owned wells require accessible names and the shared reset contract: `.appearance(false)`, `.bordered(false)`, `.focus_bordered(false)`, and `.p_0()`; the well owns fill, border, and validation. Check each helper’s interaction contract: where absent, the owner supplies a stable ID, focus/tab routing, keyboard activation, action handling, and enabled/disabled semantics.

Use Geist for application labels, forms, metadata, native diffs, and script editors. Preserve the configured terminal/Markdown code fonts and terminal cell metrics; ordinary authored HTML owns its fonts. Keep existing type roles, semantic colors, stable border/focus treatment, and rem scaling. Prefer an existing recipe before adding a token; one-consumer dimensions are module constants. Preserve fixed trailing/header columns, neutral selected/open fills, and transient action press state.

## Choose the relevant checks

Inspect the actual native rendering against the selected reference at normal and narrower widths, with long names where applicable. Check spacing, type, contrast, glyphs, clipping, and stable columns. Exercise the states changed by the task rather than every screen on every edit:

| Change | Native acceptance checks |
|---|---|
| Controls/forms/menus | Rest, hover, keyboard focus, selected/open, invalid, disabled, loading, and release; stable input/dropdown borders, component-owned keyboard focus, and reserved action slots; selectable readonly values; accessible names. |
| Navigation/checkout/diff | Long titles yield to fixed actions/counts; tree/list/compact treatments remain distinct; narrow Checkout disclosure supports focus/Escape; loading stays in its row; destination and checkout identity remain correct. |
| Tasks/composer/transcript | Expand/collapse, browse and compaction preserve preferences; minimap stays pane-anchored; session switches retain ownership; response Copy includes collapsed prose; hover/focus controls do not shift layout; inline edits preserve draft/images. |
| Source-attached confirmation | Actual trigger anchor, pane bounds, blocked background/HTML/scroll, contained Tab, Escape/Cancel focus restoration, incoming-update stability, message/thread identity. |
| Settings | Scroll and category reset, reachable navigation, nested Back/Escape source focus, contained Tab, correct switch meaning. |
| Native plans/reports/HTML | Narrow/wide multiline text, inert authored markup, exact Copy/Send payload, explicit response send, inline/expanded/external link behavior, integrated-report theme/zoom, authored HTML styling, and motion-preference updates without lost input/document state. |
| Motion/keyboard routing | Native reduced-motion static endpoints without scheduled animation frames; HTML CSS/Web Animation suppression and cooperating scripts; type spaces into affected inputs; relevant shortcuts route to the owner. |

For shared questions, approvals, tasks, subagents, or transcript behavior, name **Codex and Claude** in acceptance checks. Report each as verified, unsupported, or untested with the method or reason. Fixtures do not establish live provider-boundary behavior. Cover affected macOS/Linux/Windows paths when platform behavior changes; do not infer unobserved parity.

## Verify and maintain

Run relevant code checks and the conformance suite as required by [AGENTS.md](../../../AGENTS.md). A passing source check or worker claim cannot establish native UI correctness. Record the exact build/worktree observed, screenshot/interaction evidence, and passed, failed, or unobserved checks. Correct observed drift; if native inspection is blocked, state the blocker and unobserved checks without claiming verification. An isolated QA app may be used before the final installation; documentation/skill-only changes require neither build nor installation.

Update DESIGN.md when an approved change adds or changes an invariant, and update this skill when its procedure or checks need to change. Restoring an already documented rule needs no redundant guidance edit. Keep rules semantic. The [October 20–22 drift audit](references/drift-audit-2026-10-22.md) is optional provenance for tracing a regression or reconciling older layouts, not another design authority.
