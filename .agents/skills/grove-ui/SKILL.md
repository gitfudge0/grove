---
name: grove-ui
description: Implement and review Grove native UI, including navigation, forms, menus, settings, transcript/composer, task docks, native plans, and HTML reports, against the shared design contract and affected interaction states.
---

# Grove UI

Before implementing, mocking, delegating or reviewing Grove UI, read the
[product index](../../../product/README.md) and
[design contract](../../../product/design.md). The contract owns UI invariants;
current native owners supply exact geometry, colors and component behavior.
Explicit user direction takes precedence over default guidance.

## Establish the target and scope

Record these in existing task state, proportionate to the change; settled choices
need no reconfirmation, separate document or approval:

- Exact screen/component and owning render path. For screenshot-driven work,
  identify surrounding labels and distinguish similar headers, panels or controls
  using source and available captures before editing. Ask only if material
  ambiguity remains after inspection.
- Selected reference/variant, placement, required content and exclusions.
- Existing controls, options and interactions to preserve, and requested changes.
  A visual redesign does not authorize replacing the underlying picker or flow.
- Observable acceptance outcomes and the evidence needed to establish them.
- Selected delivery endpoint and any no-install, stop or other user constraint.

Research may reveal additional capabilities; it does not expand an approved
interface. Keep findings separate from authorized display content. Reconcile new
user direction with completed and pending work, update this task state, and
propagate it to any workers before dependent work continues.

## Load the relevant procedure

Read only the guides needed for the task. Implementation and review require the
acceptance guide as well as the relevant implementation/reference procedure.

| Work | Required guide |
|---|---|
| Native controls, layout, typography, interaction states or motion | [Native implementation](../../../product/ui-implementation.md) |
| Exploratory mock creation/revision or implementation of a selected HTML reference | [Mocks and references](../../../product/ui-mocks.md); measured-reference work also uses its linked reference workflow |
| Native implementation, review, verification or completion claims | [UI acceptance](../../../product/ui-acceptance.md), including the relevant surface checks and Codex/Claude evidence boundaries |

For GPUI mechanics, use the pinned-source procedure in
[development guidance](../../../product/development.md). Delivery follows
[AGENTS.md](../../../AGENTS.md) and the task's explicit endpoint and constraints.
Documentation/skill-only changes need neither build nor installation.

Update the design contract only when an approved change alters an invariant.
Update the owning procedure when its workflow or checks change; correcting a
conformance violation does not require a new rule or exemption. Keep Claude's
discovery entry forwarding here rather than duplicating procedures.
