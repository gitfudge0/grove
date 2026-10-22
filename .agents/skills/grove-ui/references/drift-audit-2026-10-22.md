# UI drift provenance: October 20–22, 2026

Read when changing these areas or reconciling old screenshots. This records the
reusable decisions extracted from the repository commits in the inclusive
October 20–22 window. [DESIGN.md](../../../../DESIGN.md) is normative; this
reference is evidence, not a second contract. Merge, release, packaging,
performance-only, and verification-only commits do not create new visual rules.
Changes later superseded in this window are identified below.

| Commit sources | Final invariant and contract area |
|---|---|
| `ca57463` | Navigation/controls: stable trailing columns and full-title tooltips; neutral Waiting for you; complete dense action recipes; pane-local inline-edit confirmation attached to actual Send, blocks background interaction, restores editor focus. |
| `e686213` | Diff headers: nonshrinking actions, automatically sized text actions, yielding title. |
| `0a75f31`, `4afdad3` | Composer model/effort: compact combined control, catalog-supported discrete stops, continuous drag preview/one release commit, stable inset track/thumb coordinates, reset and reduced motion. |
| `3cf3192`, `e6e8e68`, `11945e0`, `f512a02`, `244d05a`, `0f55fab` | Checkout card: session-scoped reserved content space, shared field inset, native background-generated commit flow, row-local loading/status, semantic glyphs, no branch/header/Fetch-Pull toolbar. |
| `715910f`, `1b83877`, `aa5548a`, `8d4c920` | Narrow Checkout disclosure: keyboard focus and Escape; pointer/selector coverage is verification, not a separate layout. |
| `741d4fc`, `e696c67`, `0e79012` | Checkout groups and PRs: aligned count columns, hidden zero removals, filtered category diffs, contextual PR readiness/merge confirmation and checkout identity. |
| `94a5e88`, `9c25f0b`, `fd5ae87`, `5928224`, `aaf6288` | Tasks: one dock above composer, pane-anchored minimap, no empty choice gaps; shared progress with separate tree/list treatments and trailing tree progress. |
| `36b3a93`, `bb96285`, `79b309c`, `e037ad9` | Task persistence/disclosure: survive turns and compaction; browse compaction does not overwrite preference; long-running work uses visible native task outcomes. |
| `646c18f`, `cb83b48`, `0ec337b`, `ca57463` | Inline message editing: provider-specific recoverability, preserve composer text/images, source-local confirmation tied to message/thread identity. |
| `f6e2518`, `b5ee1b5`, `30750a4` | Composer: shared Send/Stop position, immediate steering and provider-specific busy semantics without duplicated queued messages; Claude can start from composer. |
| `84e754c`, `ca57463` | Settings: centered category/body column, accessible pinned navigation, body scroll/reset, nested Back/Escape, contained Tab, shared switch tracks and user-meaningful copy. |
| `d606d9c`, `91061a9`, `155c7d0` | Transcript: literal code copy, timestamps and hover/focus actions without geometry jumps, response-level footer across work periods and collapsed prose. |
| `47995bb`, `3dedcf1`, `7fa377f`, `730ff66`, `ca57463` | HTML: macOS/Linux/Windows rendering paths; toolbar-free inline documents; integrated canvas/content sizing and theme/zoom; expanded controls, external browser links; motion preferences preserve document/input state. |
| `7bed6a5`, `fb6186d`, `6e8c34d`, `7143236`, `c55aae0`, `ef3963c` | Native plans: versioned message/revision identity and review anchors, explicit response send/error visibility, inert code/feedback, canonical payload preservation and literal multiline response layout. |
| `942c566` | macOS menu commands retain native keyboard routing; platform checks must identify the affected platform. |

The sidebar-upgrades merge `3534460` and its source commits preserve the compact
rail/flyout and focused expanded hierarchy already recorded under Navigation and
sidebar. The HTML merge `d5b25ac` carries the HTML behavior above; inspect its
non-merge source when tracing a regression rather than counting it as a new rule.

Superseded treatments: `6586b59` introduced a standalone subagent panel later
removed by `dee0ae7`; do not restore it. Earlier transcript task-diff and session
diff strips were removed by `5928224` and `ca57463`. The initial checkout branch
row, project/path/status header, and Fetch/Pull toolbar were removed by later
checkout refinements. Current approved behavior is recorded in the contract.

This audit records source inspection, not live native or Codex/Claude provider
verification. Future changes still need the affected checks from the UI skill.
