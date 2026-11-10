# UI mocks and references

Read this guide when authoring or revising a Grove mock, or implementing a
selected mock. The [UI skill](../.agents/skills/grove-ui/SKILL.md) establishes
scope, preserved controls and the selected variant first.

## Exploratory mocks

Inspect the actual owning controls and their relevant open/selected states before
mocking them. Preserve existing interactions and options unless the user has
asked to replace them; matching colors and dimensions does not establish parity.
Read [Use the owning components](ui-implementation.md#use-the-owning-components)
for native tokens, theme, fonts, catalog icons and component recipes, with
[design.md](design.md) as the semantic authority. Mark illustrative data and any
unsupported or approximated native treatment explicitly.

After each revision, open the exact artifact being delivered and inspect its
initial visible state and affected interactions. When a document has comparison
cards, a selected preview or multiple entry states, update and inspect each
representation affected by the change. A changed data object or a successfully
opened file does not establish that the requested content is visible. Link the
inspected revision; if rendering is unavailable, report that gap rather than
claiming appearance or interaction verification.

Keep exploratory HTML distinct from measured, implementation-ready references.
A selected direction does not make unsupported native treatments certified.

## Implement a selected reference

For an HTML reference that will become native Grove UI, follow the [reference workflow](ui-reference.md). Use its typed component document and native design-system export; do not infer dimensions, reconstruct controls from CSS or maintain a parallel palette. Ordinary exploratory HTML is not implementation-ready. Unsupported treatments require an extension in the existing native owner and its adapter/checks, rather than a local approximation.

1. Record the selected direction, fixture content and required states in task state. Validate the document against the current native contract; resolve unsupported properties and glyphs before generation.
2. Generate and compile the actual Rust composition. Keep application data, actions, focus ownership and provider/session identity in typed native bindings; do not inject source code into the document or fork a recipe's internals.
3. Author the measured HTML in a new revision directory. Inspect the actual reference and record the selected revision under the task's existing authorization. `init` only bootstraps inputs; `render` requires an existing frozen measured reference. Neither is a CSS approximation path.
4. Preserve the reference lock, HTML and measured payload. Capture candidates separately and run the read-only artifact check, then `ready`. A candidate must not update its own baseline. Changed design-system dependencies or intentional reference changes need a new explicit revision.
5. Inspect paired geometry/raster evidence and exercise the affected native interactions. Require the authored semantic audit as well as matching images, so omitted or changed content cannot be mirrored into the reference. Report failed, unsupported and untested cells explicitly; never broaden a tolerance or regenerate the baseline to obtain a pass.

The workflow's supported contexts and certification bounds are part of acceptance. Keep Codex and Claude fixture evidence separate from live provider checks, and identify actual platform coverage. Generated output and a certificate never replace the affected checks in the linked acceptance guide or authorize implementation beyond the current task.

Use [native acceptance](ui-acceptance.md) for application integration evidence.
