# GPUI development guidance

Use this when changing GPUI bootstrap, entities, actions, focus, async work,
custom elements, text rendering or platform behavior. For Grove UI semantics
and native acceptance use the [design contract](design.md) and
[Grove UI skill](../.agents/skills/grove-ui/SKILL.md).

## Verify the pinned source

Read the exact Zed revision in [Cargo.toml](../Cargo.toml) and its resolved
[lockfile](../Cargo.lock). Inspect the matching local Cargo Git checkout, not
remembered APIs or examples from another revision. The local vendored
[gpui-component provenance](../vendor/gpui-component/README.md) records its pin
and Grove patches; keep compatible revisions together when deliberately updating.

Search the pinned checkout for the relevant owner:

- `gpui_platform` and `gpui/examples/hello_world.rs` for platform/bootstrap setup.
- `gpui/src/_ownership_and_data_flow.rs` for entities, updates and events.
- `gpui/docs/key_dispatch.md` for action contexts and focus-path dispatch.
- `terminal_view` for custom Element and shaped-text patterns.
- The actual Input implementation for editing/focus behavior.

Upstream examples can drift too: compile any borrowed pattern against this
checkout. Old copied snippets and claims marked “EXACT” without a pinned revision
are not a verified API reference. Inspect signatures before writing code.

## Implementation checks

- Keep subscription/task handles alive for the required lifetime; detaching or
  storing them is a deliberate ownership choice. Notify after state changes that
  need rendering. Avoid reentrant entity updates; defer or route events through
  the owning entity where necessary.
- Keep focus ownership explicit when mounting/dismissing views. Give interactive
  elements stable identities and verify actual keyboard routing; action binding
  presence alone is not input evidence.
- Use GPUI executors for UI/entity work. Keep blocking work off the foreground
  executor; do not introduce a separate runtime without an explicit bridge.
- Preserve native terminal cell metrics. Use the shared rem-scaled Grove recipes
  for application chrome, not copied pixels or a parallel widget implementation.
- Follow the existing custom-element or virtualized-list owners for large feeds;
  inspect their layout/paint contracts before changing them.

## Validation

Run relevant formatting, focused tests and the affected build. Native fixture
checks and live provider-boundary checks prove different things: report the
method, platform and provider actually exercised. Follow
[verification guidance](verification.md), [AGENTS.md](../AGENTS.md), and the
[Grove UI skill](../.agents/skills/grove-ui/SKILL.md) for affected interaction checks.
