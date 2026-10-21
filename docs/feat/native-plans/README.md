# Native plans

Grove's Codex and Claude sessions accept the same versioned native plan artifact.
This is separate from the `update_plan` / `TodoWrite` progress checklist.

When Grove capability guidance is present, write a `.grove-plan.json` document,
validate it, atomically rename it into place, then link its absolute local path
in a **completed assistant response**. Do not stream JSON into a transcript fence.
Keep the `plan_id` stable and use a new `revision_id` for every changed document
or asset. Stable claim, decision, call, state and layout IDs identify feedback.
Reusing a revision identity with different content is rejected.

```sh
cargo run -p grove-core --example validate_plan -- /absolute/checkout/plan.grove-plan.json
```

Then deliver `[Review the plan](</absolute/checkout/plan.grove-plan.json>)`.
[example.grove-plan.json](example.grove-plan.json) exercises the supported
vocabulary and is checked against the importer in tests. Assets resolve relative
to the document and must remain inside the session's registered checkout roots.
The importer snapshots the document and assets; changing or deleting source files
does not change an imported review. Invalid imports show an error and explicit
Retry while preserving earlier revisions.

The schema has `schema_version: 1`, `plan_id`, `revision_id`, `title`, `sources`
(`attribution`, `text`), `changes` (`files`, `new`, `changed`, `deleted`), `claims` and `scope`.
Each claim has `id`, `text`, optional `exhibit`, `html_links`, `decisions` and
`children`. Keep claims at most three levels deep and five children per parent.
Each decision has `id`, `prompt`, `options` (`id`, `label`, `description`) and an
optional `default` containing an option ID. Recommendations are not accepted
until the reader explicitly chooses them; an own answer remains free text.

An exhibit uses a `kind` discriminator:

| Kind | Content |
| --- | --- |
| `code` | `language`, `text`, optional `source` context |
| `schema` | `text` |
| `calls` | `entries` containing `id`, `title`, `detail`, optional `source` |
| `state_machine` | `states` (`id`, `label`, optional typed `screen`), `initial`, `transitions` (`from`, `to`, `label`) |
| `image` | local raster `path` and meaningful `alt` |
| `layout` | `nodes` containing `id`, `kind`, `text`, `children`; kinds are `row`, `column`, `surface`, `text`, `control` |
| `html_link` | local HTML `path`, `label` |

Use one exhibit per claim. Supporting `html_links` contain `path` and `label`.
Layouts are constrained illustrative primitives with Grove styling; they accept
no CSS, scripts, callbacks or app behavior. Code is inert read-only text. HTML
opens only on an explicit action in the existing separate sandboxed renderer;
return restores the native claim. Network and file requests remain blocked.
Keep documents below 1 MiB, individual text fields below 64 KiB and total decoded
raster assets below 20 MiB. Unsupported versions, unsafe paths, invalid IDs,
missing assets and conflicting revisions fail validation.

Readers expand claims, choose answers, write comments and inspect **Review
response** before **Send response**. Feedback is bound to this plan revision and
session, persists across restart, and uses the normal provider queue while busy.
Sending feedback does not authorize implementation or change permission mode.
The existing composer draft remains independent. A confirmed send cannot be
sent again; uncertain delivery after restart is not automatically replayed.
Copy response is available when dispatch cannot be used. Older reviews remain
on their original revisions; feedback is never silently copied to a changed
target. Compare and explicitly re-enter feedback on the new revision if needed.

This repository owns the authoring example, validator and provider capability
instructions. Installed external `fudge:plan` skill copies are not modified.
Agents running that skill in Grove should select this native contract when the
capability guidance is present. Outside Grove retain the canonical packed HTML
workflow. Existing HTML reports and their local feedback are not converted.

## Verification

Core tests cover validation, immutable snapshots, revision conflicts, persistence
compatibility and duplicate delivery. Native fixture tests exercise native
rendering and interaction; they do not prove a live provider boundary.

For **each of Codex and Claude**, start a real session, ask it to generate and
link a valid native plan, expand and answer a decision, add a comment, and send
the reviewed response. Verify the same session receives one response, both idle
and while busy, while a pre-existing composer draft remains intact. Revise the
plan, switch sessions, restart Grove, and confirm revision-specific feedback is
retained. Confirm invalid import/retry, linked HTML/return and legacy HTML.
Record each provider as verified, unsupported or untested with the actual method.

Native visual checks cover desktop/narrow widths, long content, keyboard
disclosure and Escape/focus return, System/Dark/Light, zoom, action release,
image sizing and session isolation. Record the exact inspected build and host
platform; unexercised macOS, Windows and Linux behavior remains untested.
