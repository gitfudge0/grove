# Native UI implementation

Read this guide when implementing or changing Grove native UI. The
[UI skill](../.agents/skills/grove-ui/SKILL.md) establishes scope and the exact
target first; [design.md](design.md) owns UI invariants.

## Use the owning components

Inspect the owning native view plus [tokens](../src/views/tokens.rs), [theme](../src/theme.rs), [components](../src/views/components.rs), and [icons](../src/icons.rs). Match a screenshot/mock to these primitives; do not copy CSS values or invent GPUI APIs. Use the contract’s control decision table: complete dense/form action recipes, form/project field wells, question/settings controls, attached validation, selectable readonly values, and shared switches/tooltips/status marks. Do not fork their shape locally. Native vendor inputs inside owned wells require accessible names and the shared reset contract: `.appearance(false)`, `.bordered(false)`, `.focus_bordered(false)`, and `.p_0()`; the well owns fill, border, and validation. Check each helper’s interaction contract: where absent, the owner supplies a stable ID, focus/tab routing, keyboard activation, action handling, and enabled/disabled semantics.

Use DM Sans for application labels, forms, metadata, native diffs, and script editors. Use JetBrains Mono for terminal/Markdown code and preserve terminal cell metrics. Audit every configured font path, including vendor theme defaults, diagrams, embedded HTML and dynamically inserted markup; authored HTML font choices must normalize to DM Sans or JetBrains Mono while retaining monospace semantics. Keep platform glyph fallback for unsupported Unicode. Check the bundled font asset list and licenses, and remove replaced font assets. Keep existing type roles, semantic colors, stable border/focus treatment, and rem scaling. Prefer an existing recipe before adding a token; one-consumer dimensions are module constants. Preserve fixed trailing/header columns, purposeful selected and popover-anchor fills, transparent inline disclosures, and transient action press state.

## Review interaction highlights

Tooltips are opt-in, not a default control treatment. Add one only where it supplies needed context, identifies an otherwise ambiguous icon-only action, or exposes information that cannot be shown in place. Do not duplicate visible labels. Preserve accessible names independently, and use the shared tooltip surface when a tooltip is required. Check the owning view and explicit contract before retaining or adding one.

Classify each affected target as an inline disclosure/link, bounded action, navigation or selectable option, popover anchor, field, or static metadata before styling it. Apply the interaction state table in `product/design.md`: expansion is not selection; inline disclosures stay transparent on hover and expansion, while keyboard focus remains independently visible. Keep a fill within the actual target and prevent nested actions from triggering unrelated highlights. Confirm disabled controls reject activation and active pointer feedback, and that release/cancel clears press feedback. Use shared semantic colors and recipes rather than local color or geometry forks.

Inspect rest, pointer hover, keyboard focus, expanded/open, selected, pressed/released, disabled/loading, and relevant combinations in the actual native view. Exercise normal and narrow widths, long labels, light and dark themes, and changed rem zoom as relevant. Check that focus remains visible and unobscured, distinguishable against adjacent colors in every affected theme and selected state, and that visual state is also conveyed semantically. Document measured contrast and real native evidence; source inspection and fixture assertions alone do not establish visual compliance. Report unobserved provider and platform paths explicitly.

## Choose motion deliberately

For every new or changed UI, identify meaningful state changes and select the suitable shared motion before implementation. Use Make room when entering/exiting content displaces neighbors, restrained fades for transient surfaces/replacement, and state transitions for feedback. Cover both entrance and dismissal, retain passive exit content through collapse, and keep keys stable across redraws. Static treatment needs a concrete reason (reduced motion, unchanged content, or immediate input/focus/output/layout/resizing feedback). Check interruption, repeated updates, neighbor movement, and reduced-motion immediate endpoints without scheduled animation frames. Avoid decorative motion.

For the changed surface, follow [native acceptance](ui-acceptance.md).
