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

The schema has `schema_version: 1`, `plan_id`, `revision_id`, `title`, optional `summary` (the intended outcome), `sources`
(`attribution`, `text`), `changes` (`files`, `new`, `changed`, `deleted`), `claims` and `scope`.
Each claim has `id`, `text` (a concise heading), optional `detail` (the explanation), optional `exhibit`, `html_links`, `decisions` and
`children`. Keep claims at most three levels deep, with at most five top-level
claims and five children per parent. Claim, decision, call entry, layout node
and state IDs are nonempty and globally unique; option IDs are scoped to
their decision.
Each decision has `id`, `prompt`, `options` (`id`, `label`, `description`) and an
optional `default` containing an option ID. Recommendations are not accepted
until the reader explicitly chooses them; an own answer remains free text.

`sources` is an array of quoted evidence objects, not path/URL reference objects.
`changes` is one object of nonnegative integer counts, not an array of file edits.
`scope` is an array of strings. Every layout node requires `text`, including
containers (use `""` when they have no text); `children` may be omitted.
The shared provider guidance includes a self-contained JSON example covering
these shapes, checked by a test through the production importer. JSON syntax
checks alone do not prove a plan can be imported. The validator command above
runs from the Grove repository; it is not a globally available provider tool.

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

The transcript retains an Open plan entry. The selected conversation's right tools
also show **Plan** above Agents, listing the newest revision of each plan. Hover
for an anchored peek; click or use the keyboard to keep it open. The peek shows
the authored outcome, main changes and one editable decision. **Next decision**
advances through unanswered choices. **Expand** fills the conversation area with
a single readable document while workspace navigation remains available. Claim
disclosure and revision-specific answers/comments persist between surfaces.

Both surfaces keep their action bar outside the scrolling body. **Review
response** replaces the document with the exact feedback that **Copy response**
and explicit **Send response** deliver. **Back to plan** preserves the review.
Feedback is bound to its plan revision and session, persists across restart,
and uses the normal provider queue while busy. Sending feedback does not
authorize implementation or change permission mode.

**Approve & implement** is a distinct explicit user action, available in peek
and expanded views. It sends approval and chosen answers to the same owning
agent/session, asking it to implement that immutable revision. A valid explicit
option or nonblank own answer is required for every decision; recommendations
do not count. Plans without decisions can be approved immediately. Superseded
revisions cannot be approved. Approval and feedback have separate delivery
journals, so sending feedback does not consume approval. Busy approval is queued
and revalidated before dispatch; duplicate or uncertain deliveries are not
replayed. Permission and sandbox policy remain unchanged.

The existing composer draft, images and references remain independent. Failed
delivery stays visible; Copy response remains available when dispatch cannot be
used. Older reviews remain on their original revisions; feedback is never
silently copied to a changed target. Compare and explicitly re-enter feedback
on a new revision if needed. Version 1 documents without summary/detail remain
valid and display only their authored content.

This repository owns the authoring example, validator and provider capability
instructions. Installed external `fudge:plan` skill copies are not modified.
Both providers receive shared workflow guidance on new and resumed sessions:
Grove controls supported presentation and response delivery even when a skill
prescribes a different format or handoff. Keep the skill's substantive planning
method, but deliver interactive plans and decision reviews using this native
contract, a Markdown artifact link, and **Review response** / **Send response**. User approval uses the separate
**Approve & implement** control; generated content and comments never approve.
A bare or backticked path does not import a plan. Ordinary short prose plans need
no artifact. Explicit user preferences, project domain constraints, safety and
permission boundaries still apply. Legacy HTML is a fallback for an explicit user
request or content the native format cannot represent; explain that fallback.
Outside Grove retain the canonical packed HTML workflow. Existing HTML reports and their local feedback are not converted.

## Verification

Core tests cover validation, immutable snapshots, revision conflicts, persistence
compatibility and duplicate delivery. Native fixture tests exercise native
rendering and interaction; they do not prove a live provider boundary.

For **each of Codex and Claude**, start a real session, ask it to generate and
link a valid native plan, expand and answer a decision, add a comment, and send
the reviewed response. Also explicitly approve a safe disposable plan and verify
implementation begins in the same session with chosen answers. Verify one
delivery per action, both idle
and while busy, while a pre-existing composer draft remains intact. Revise the
plan, switch sessions, restart Grove, and confirm revision-specific feedback is
retained. Confirm invalid import/retry, linked HTML/return and legacy HTML.
Record each provider as verified, unsupported or untested with the actual method.

Native visual checks cover desktop/narrow widths, long content, keyboard
disclosure and Escape/focus return, System/Dark/Light, zoom, action release,
image sizing and session isolation. Record the exact inspected build and host
platform; unexercised macOS, Windows and Linux behavior remains untested.
