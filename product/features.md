# Feature behavior

Read the relevant section when changing file categories, plans or HTML reports.

## Change categories

Grove groups changed files under Code, Tests, Docs, and Config. Unrecognized file types appear under Uncategorized. Classification uses paths and filenames, never an AI guess about the intent of a change. A file belongs to exactly one parent and optionally one subcategory. Parents include files without a subcategory; there is no Other row.

Populated parents appear in the checkout card, even when only one parent is present. The arrow expands populated subcategories. Clicking a parent or child opens its filtered diff. Expansion is local to the selected checkout. New file and mapping changes are picked up by the existing status refresh.

Built-in subcategories recognize Tests → Unit, Integration, End-to-end, Fixtures & snapshots; Config → Dependencies & lockfiles, Build & tooling, CI/CD, Deployment & infrastructure, Environment & app settings; Code → Scripts & automation, Assets. Generic test filenames remain under Tests without guessing unit versus integration. Unit and integration directory names require test context. Bare JSON/YAML/TOML and arbitrary dotfiles do not imply Config. Test and docs context takes priority over extensions.

### Optional repository mappings

Create `.grove/change-categories.json` in each checkout. No mapping is required. For example:

```json
{
  "rules": [
    { "path": "src/views/special.rs", "category": "code", "subcategory": "Special UI" },
    { "path": "src/views/", "category": "code", "subcategory": "UI" },
    { "path": "resources/settings.json", "category": "config" }
  ]
}
```

Rules run in array order, before built-in conventions; the first match wins. Paths are case-sensitive, relative to the checkout, and use `/`. A trailing slash matches that directory's descendants; otherwise the match is exact. `src/views/` does not match `src/views-old/`. Globs and absolute or parent-relative paths are unsupported. Put specific rules before broader directory rules. Valid categories are `code`, `tests`, `docs`, `config`, and `uncategorized`. Omit `subcategory` to keep files directly in the parent. Custom subcategory labels must be nonempty, contain no control characters, and be at most 64 bytes.

Files are limited to 64 KiB and 256 rules. A missing mapping uses built-ins. An unreadable or invalid mapping logs a warning and falls back to built-ins as a whole; it never removes changed files from the summary or diff. Overrides are read as data and execute no commands. Commit the mapping to share it with teammates and future worktrees.


## Native plans

Plans are readable, immutable Markdown documents in the conversation. Author a
`.grove-plan.json` envelope and link its absolute path in a completed assistant
message. Keep one stable plan ID and use a new revision ID whenever its contents
change:

```json
{
  "schema_version": 1,
  "plan_id": "search-performance",
  "revision_id": "r1",
  "title": "Make workspace search responsive",
  "markdown": "# Make workspace search responsive\n\nShow useful results promptly.\n\n## Approach\nCompare direct scans with a background index.\n\n## Verification\nMeasure query latency and compare result accuracy.",
  "decisions": [
    {
      "id": "approach",
      "section": "Approach",
      "prompt": "Which approach should the prototype test?",
      "options": [
        {"id": "scan", "label": "Direct scan", "description": "No persistent index; repeated query work"},
        {"id": "index", "label": "Background index", "description": "Fast repeated queries; freshness must be visible"}
      ]
    }
  ]
}
```

Keep finalization metadata in the JSON envelope, not in the readable Markdown. Show previous answers as labeled prose or a table. Grove presents locally generated plan-update and change-decisions requests as readable transcript summaries while retaining their exact provider payloads.

Lead with the outcome and scope, make decisions and risks visible, and state
concrete success criteria. Choose representations that suit the question:
before/after tables for behavior changes, a concise flow for sequencing, and
comparison tables for tradeoffs. Supporting technical detail can follow those
essentials. Do not author a separate HTML plan or claim/exhibit tree. Limits
remain 1 MiB per document and 64 KiB per text field.

### Native overview and expandable detail

For an implementation plan, an optional `presentation` object makes the first read compact while preserving complete Markdown detail:

```json
{
  "outcome": "Connect your agent without losing the current draft.",
  "flow": [
    {"title": "Choose agent", "detail": "Codex or Claude"},
    {"title": "Sign in", "detail": "Provider browser consent"},
    {"title": "Return", "detail": "Verify the account"}
  ],
  "included": ["Account sign-in and recovery"],
  "deferred": ["API-key entry"],
  "risks": ["Browser completion must be verified with the provider."],
  "success_criteria": ["Cancel preserves the conversation draft."],
  "revision_changes": []
}
```

This is part of the existing version-1 plan envelope, alongside `markdown` and
`decisions`, not a separate plan format or approval authority. The outcome permits
1200 bytes; flow permits eight steps with titles of 160 bytes and details of 500
bytes. Each list permits twelve nonblank entries of 1000 bytes. Risks and success
criteria are required and nonempty. Describe a lack of known material risk
explicitly rather than hiding the risk section. Import rejects invalid presentation
and presentation without Markdown. The overall document/text limits still apply.

The native conversation shows outcome, flow, scope, risks, success criteria and
all decision choices before individually expandable Markdown detail sections.
Option descriptions state consequences and tradeoffs. A finalized revision can
show `revision_changes` as an authored summary; it is not an automatic diff.
Update both layers when answers change scope or success criteria. Existing
finalization and exact-revision approval checks remain authoritative, including
the requirement for updated Markdown. Copy and the read-only reference include
all presentation content and complete Markdown, irrespective of fold state.
Older envelopes without this optional object retain their full Markdown reading
experience. Folding uses local per-revision UI state and resets when the native
view is recreated; it does not persist an answer or change the plan snapshot.

Top-level `decisions` are optional. A decision's optional `section` names the
Markdown heading beside which it belongs. Recommendations are not selected
answers. Every decision supports **Write my own answer** through the existing
conversation composer; there is no separate plan input. **Discuss** also uses
that composer with the plan revision and section as context. Entering answer mode
preserves the ordinary composer draft; saving or canceling restores it. Saving a
custom answer records literal user text and never sends implementation approval.

Resolve every decision, then choose **Update plan with decisions**. This delivers
an update request to the owning session and snapshots the exact option labels or
custom answers. The agent must incorporate those answers into the actual
Markdown approach, scope and success criteria, then publish a new revision with
no unresolved decisions and explicit provenance:

```json
{
  "schema_version": 1,
  "plan_id": "search-performance",
  "revision_id": "r2",
  "title": "Make workspace search responsive",
  "markdown": "# Make workspace search responsive\n\n## Approach\nPrototype a background index with visible freshness.\n\n## Verification\nCompare accuracy with the existing scan and measure warm-query latency and indexing cost.",
  "finalized_from": {
    "revision_id": "r1",
    "answers": {"approach": "Background index"}
  }
}
```

Only a current finalized revision can **Approve & implement**. Selecting all
answers is not enough: a decision-bearing draft must first be incorporated into a
new revision. Import and approval validate provenance against the actor-owned
submitted-answer snapshot; omitting decision fields cannot bypass this boundary.
A plan with no decisions can be approved directly unless it still owes
finalization of a preceding decision revision. If a question or its options change
meaningfully, its answer must be given again; unchanged decisions may retain
valid answers. Earlier revisions remain immutable references.

**Change decisions** revokes the current finalized revision's eligibility for
implementation and requests a new decision draft from the owning agent. The
previously finalized choices remain context; explicit answers are preserved only
when the new question and options are semantically unchanged. Changed questions
require fresh answers. Even if every preserved answer remains valid, the new
decision draft must be submitted and incorporated into another finalized revision
before implementation can be approved. Failed or uncertain delivery of the
change request never restores the old revision's approval eligibility.

The separate **Review response** screen and redundant **Plan ready** response
card are removed. The right-panel **Plan** section lists the latest revision of
each plan, including approved plans, as reading references. The conversation is
the response surface; the peek does not introduce another answer form. Existing
queueing, duplicate protection, uncertain-delivery recovery, composer draft
preservation and exact-revision approval remain enforced. Approval stays in the
owning session and never changes sandbox or tool permissions.

Legacy structured plans remain importable and readable. Their decisions follow
the same explicit-answer and new-revision finalization rules; their stored
documents are never rewritten.

Verification must name both **Codex and Claude**. Core tests establish import,
persistence, revision and delivery contracts; native fixtures establish only the
exercised UI behavior. Neither establishes live provider-boundary behavior. The
opt-in `plan_finalization_smoke` example exercises live provider generation,
mixed selected/custom decision delivery, finalized import, duplicate suppression,
draft preservation and persisted reload in a disposable cwd/config. It checks
approval readiness on cloned stores and never sends implementation approval.
Inspect actual native rendering and interactions separately, including narrow
widths, zoom, focus, draft restoration and read-only plan references; report
unavailable platforms and live checks explicitly.


## Explicit HTML visual publication

Published HTML artifacts appear as single-line transcript rows with a truncating
title, muted Artifact label and two icon-only actions: Open artifact and Open in
browser. The actions retain accessible names and tooltips. Every presentation mode opens live HTML in the
dedicated document viewer; the transcript contains no live HTML or automatic
thumbnail capture. Back restores the conversation and the viewer retains input
and camera state when reopened. The `presentation` hint (`inline`, `dashboard`,
`canvas`, `document`) and width/height remain authoring and agent-preview metadata.
Optional `preview_region` supplies a scroll origin or a cooperating canvas frame
through the one-time `grove:initial-view` event. Agent-requested native screenshot
previews remain available without publishing an artifact.

New Codex and Claude conversations publish HTML artifacts through the private
`grove_visuals.render_html` tool. Fenced HTML stays code and ordinary local HTML
links open the file viewer; Open in browser links remain external. Older saved
conversations without `html_policy` retain legacy automatic previews. Forks
inherit their source policy, resume preserves it, and independent new conversations
use explicit publication. Native Markdown plans remain a separate feature.

`render_html` accepts exactly one of complete inline `html` or an absolute local
`path`, a title and a stable `request_id`. An optional `visual_id` creates a new
immutable revision; it never edits an earlier reply. Each call must match one
observed root tool event in the active owning turn. Calls without reliable root
ownership, including inherited child-agent calls, fail with an explicit error.
Read-only side conversations receive no visual tools. User MCP sources are
preserved when Grove registers its separate authenticated session-local endpoint.

Publication commits the offline document and reference atomically before returning
success. The tool event anchors the visual; normal tool receipts remain available,
and visuals remain visible when work details are collapsed. Saved references load
private bundles after restart, fork and source deletion. Recovery reconciles a
commit whose acknowledgement was interrupted, provided its source event remains.
Rewinds hide removed anchors. Conservative cleanup scans saved records, archives
and journals, retaining active-session retry records and recent orphan commits;
unknown/unreadable roots prevent sweeping. Private bundles are not repository artifacts.

Source documents are limited to 2 MiB and prepared bundles to 10 MiB. Authorized
local PNG, JPEG and WebP images are embedded after root, regular-file, identity,
size and decoding checks (at most 64 assets). Inline CSS, JavaScript, fonts and SVG;
external dependencies and network/file loading are unsupported. Direct host reads
require positively acknowledged effective bypass permissions; otherwise pass
inline HTML with data assets. An MCP registration never broadens provider access.

`preview_html` accepts the same inputs and returns a PNG image, source digest,
viewport/content dimensions, actual raster dimensions and bounded available
console diagnostics. It publishes nothing. Capture uses isolated nonactivating
native webviews with the same offline preparation, fonts, theme and motion policy
as inline rendering. Widths 240–1600 and heights 80–2000 are supported; use 360 and
728 for narrow/desktop checks. Capture concurrency, readiness and timeouts are
bounded. Cancellation/teardown discards the temporary document; unavailable native
capture does not disable publication. No browser download is required.

macOS capture uses WebKit snapshots, Linux uses WebKitGTK snapshots, and Windows
uses WebView2 screenshot capture. Platform source coverage is not runtime parity;
see `verification.md` for observed and unavailable checks. A headless core actor has
no native capture loop and returns an unavailable error rather than a fake image.

## Integrated HTML reports

An agent can return a report that reads as part of the conversation rather than
a framed document. Ordinary HTML previews are unchanged.

### Contract

A complete HTML document (`<!doctype html>` or `<html>` through `</html>`, at
most 2 MiB) opts in with the exact comment `<!-- grove:integrated-report -->`.
In legacy conversations it reaches the transcript through a closed fenced
`html` block or a Markdown link to a local `.html` file. Explicitly published
visuals receive the report marker automatically during preparation.

Grove injects one `<style id="grove-report-theme">` element and replaces its
contents whenever appearance or UI zoom changes, without reloading the document.
Colours come from `src/theme.rs`; sizes come from `src/views/tokens.rs` at the
current zoom.

| Variable | Source |
|---|---|
| `--grove-bg`, `--grove-fg`, `--grove-muted` | `BG`, `FG`, `FG_DIM` |
| `--grove-surface`, `--grove-border` | `BG_HOVER`, `BORDER` |
| `--grove-info`, `--grove-success`, `--grove-warning` | `BLUE`, `GREEN`, `AMBER` |
| `--grove-body`, `--grove-title`, `--grove-display` | `TEXT_BODY`, `TEXT_DISPLAY`, `TEXT_DISPLAY_LG` |
| `--grove-gap`, `--grove-section`, `--grove-radius` | `SPACE_LG`, `SPACE_20`, `RADIUS_GROUP` |
| `--grove-font` | Bundled DM Sans UI family |

The injected rules also set the root background, colour, font and size, zero the
body margin, and hide root overflow while the report is inline.

Claude and Codex learn this contract from Grove's injected session guidance
(`crates/grove-core/src/reports.rs`). `examples/integrated-report.html` is a
sample.

### Behaviour

- Legacy inline reports have no frame, title or editing toolbar; Open in browser
  remains visible. Published artifacts use the single-line artifact row described
  above. The dedicated viewer retains Preview, Source, Reload and return controls.
- The native view is sized to the measured body height, capped at eight times
  the standard preview height. Expand shows longer reports in full.
- Wheel input over an inline report scrolls the conversation. Expanded, the
  document scrolls itself.
- Document state (tabs, filters, inputs) survives Expand and Source toggles.
  Reload and restart reset it. Opening in a browser uses the original file,
  which does not receive Grove's theme.

### Platform boundaries

Height is pulled by the host: every 100 ms the view requests a read-only
evaluation of the body height and receives a single finite number.

| Platform | Wheel forwarding |
|---|---|
| macOS (WKWebView) | The web view passes wheel events to the GPUI host view. No document messages. |
| Linux (WebKitGTK, X11) | GTK scroll events are captured in-process and dispatched into the GPUI window during the GTK pump. No document messages. |
| Windows (WebView2) | WebView2 is out of process and does not return wheel input, so a host script posts wheel deltas. Web messages are enabled for this alone: the host accepts only finite, clamped wheel deltas from a visible inline report and ignores every other message. |

The existing sandbox otherwise applies unchanged: CSP, blocked network and file
requests, no host objects, no workers, no popups.

### Authoring limits

Avoid viewport-height layouts, inner scroll areas, fixed or sticky positioning,
body margins, and `!important` overrides of the injected rules. Embedded DOM text uses bundled DM Sans, with JetBrains Mono for semantic or authored monospace text. Font choices are normalized, including dynamically inserted text. Platform glyph fallback remains available for unsupported characters. Text drawn inside authored canvas/bitmap assets is intrinsic content.

### Verification

```sh
cargo test --bin grove views::inline_html::tests
clang -fobjc-arc -fblocks scripts/verify-integrated-report-webkit.m \
  -framework AppKit -framework WebKit -framework QuartzCore -o /tmp/grove-report-webkit-test
/tmp/grove-report-webkit-test examples/integrated-report.html
```

The macOS harness checks measured growth and shrinkage independent of viewport
height and the absence of script-message handlers. CI runs the equivalent
WebKitGTK and WebView2 harnesses.

## Describe changes

Checkout offers **Describe changes** in the always-visible overflow menu beside
the **Local changes** and **Pull request** section headings. Placement establishes
scope: local changes include staged, unstaged and nonignored untracked files
against HEAD; the PR uses GitHub's base-to-head diff. No source dropdown or
separate tool action rows are shown. The scopes are never combined. A command or Checkout action creates a dedicated,
retained conversation with the selected provider and checkout, preserving the
originating composer draft. A repeated invocation while that source is generating
opens the active session. The immutable diff is captured and saved before the
session's normal provider turn receives it. The validated final result opens in
the native reader; its Conversation action returns to the same retained conversation.

Grove keeps one canonical private JSON result per PR identity or local checkout
under its configuration directory's `change-descriptions/sources/`. Stable hashed
paths are independent of session, model and snapshot revision. Session links and
captured jobs are separate recovery metadata. Regeneration atomically replaces the
same result, with a locked source claim and unique request identity preventing an
older generation from overwriting a newer one, even after restart. Acknowledged
cancellation/failure retires only its own request and preserves the previous result.

A saved result exposes **Open description** in its source section's overflow
menu, including after local edits disappear or a PR is detached. It reopens the
saved page with source context and an **Out of date** status when stale; it does
not run generation. Regeneration is available inside the reader. Both links and
results survive restart.
Reopening reads the canonical result, and freshness checks show stale or unavailable
source state. Interrupted jobs may import their exact completed conversation result;
Grove never resubmits an uncertain request automatically.

The native reader shows a behavior-focused summary, file walkthrough grouped by
purpose, optional Mermaid flow and explicit coverage. File expansion exposes its
summary and the captured read-only patch, not a subsequently changed live file.
Binary, oversized, missing and model-omitted patches remain visible as coverage
limits. No test execution is implied. Background freshness checks mark changed or
unverifiable snapshots; stale results remain readable/copyable but cannot publish.
Cancellation interrupts the owning provider turn and preserves the previous
saved description. Saving a completed result is a short finalization phase with
Cancel disabled.

**Copy Markdown** copies the generated result. **Update PR description** opens an
editable native preview, preserves existing human context and the PR title, and
publishes only after explicit confirmation. Publication rechecks checkout, PR
head/base and the body captured for the preview. A competing write after that
preflight cannot be atomically excluded by GitHub's body-update API. Successful
updates retain their acknowledged body for later freshness checks. GitHub.com is
supported; enterprise hosts report an unsupported URL rather than guessing.

### Shared Codex and Claude skill

Grove bundles `describe-changes/SKILL.md`. In either provider's Grove composer,
`/describe-changes` or `$describe-changes` opens the same native workflow. Optional
`local`, `pr`, or a GitHub.com pull request URL selects a scope. Explicit URLs
describe that PR without switching or attaching the checkout. Dashboard commands
are intercepted before catalog discovery or activation of the originating draft;
the new session uses the Dashboard composer's selected provider and checkout. Without an argument, use
local changes. Section menus provide their source explicitly; command source
arguments remain available for precise selection.
Completions reserve this name, and Grove intercepts the whole command before
submitting it to the originating provider. The new session receives a captured-diff
generation prompt with a unique durable request identity. The command draft stays
intact; attachments and quoted
references are rejected with an explanation and preserved. Invalid arguments show
usage. This is a user-invoked Grove skill, not an autonomous model tool; provider
global skill directories remain unchanged, and read-only side conversations do
not receive it. Codex and Claude use their normal authenticated conversation
transports and selected model/preferences. The generation prompt instructs the
provider to use only the captured diff, without tools or repository writes; this
skill does not expand the session's permissions. Later conversation turns remain
ordinary user-directed conversation.


## Built-in prompts

Built-in writing prompts support Grove actions for Codex and Claude. Settings does
not expose a Prompts category or prompt editor. Existing saved overrides remain
compatible with future generations and Grove-managed workflow requirements.
Create PR generates title and Markdown body from the published branch comparison,
then opens the existing editable GitHub form; generation does not submit a PR.

Describe changes accepts optional extended Markdown content for headings,
paragraphs, lists, tables, blockquotes, code and file links, rendered by Grove's
native Markdown components. The existing Mermaid field provides diagrams.
Snapshot context, file walkthrough and honest coverage remain visible independently.
Copy Markdown and PR update previews include extended content. Older saved results
without extended content remain readable. Prompt changes never rewrite saved results.


## Review changes

Checkout's source-specific **Review changes** menu action, `/review [local|pr|PR-URL]` and
`$review [local|pr|PR-URL]` work from Dashboard and either Codex or Claude composer
before catalog discovery or source draft activation. Local scope captures staged,
unstaged and nonignored untracked changes against HEAD. PR scope captures the
attached or explicitly supplied GitHub PR against its base. These scopes never
combine. An omitted command scope uses the captured source in a retained review
session, otherwise local changes. Invalid arguments and attached images/references are rejected with the
source draft preserved.

Each request creates a dedicated retained conversation using the selected
provider, model and normal session preferences, or reopens its already-active
request for the same source. The originating conversation is never submitted.
Grove captures a bounded immutable diff before the initial provider turn. The
initial prompt asks for read-only analysis without tools, tests, publication or
repository writes; this is an instruction within an ordinary session, not a
separate tool-disabled sandbox or expanded permission grant. Later conversation
turns are ordinary user-directed turns.

The reader presents introduced, actionable P0–P3 findings with concrete triggers,
consequences, rationale, next steps and exact captured old/new-side line ranges.
Paths and ranges must match complete supplied patches and overlap changed lines.
Coverage names omitted, partial and model-unreviewed files. No findings means no
issues were identified in the reviewed patches, not merge approval or a guarantee
of correctness. Captured diff navigation and Copy Markdown retain their snapshot.
Saved custom Review writing instructions can change the prose;
Grove-owned schema, evidence and coverage validation remain mandatory.

One private atomic canonical result per source replaces its predecessor only
after validation. An **Open review** entry remains in the corresponding source section's overflow
menu, even if local changes or the attached PR no longer exist. Links and results survive restart. Conversation opens the same
retained session; only captured request transport and reserved `grove-review`
blocks are hidden from the visible transcript, remaining in model/recovery data.
Ordinary later answers are never imported as a new original review. Stale results
show **Out of date** and remain readable. Regeneration preserves the previous
result on failure/cancellation; Saving review rejects cancellation until its
atomic save finishes. Review does not automatically publish GitHub comments or
apply fixes.
