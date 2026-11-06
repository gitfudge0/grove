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

Plans render inline as Markdown. Author a `.grove-plan.json` envelope and link its absolute path in a completed assistant message:

```json
{
  "schema_version": 1,
  "plan_id": "search-performance",
  "revision_id": "r1",
  "title": "Make search responsive",
  "markdown": "# Make search responsive\n\nMove indexing off the input thread.\n\n## Approach\n1. Measure the current path.\n2. Move indexing into a worker.\n3. Ignore stale results.\n\n## Verification\nCompare matching and ranking; measure input-to-paint latency."
}
```

The Markdown field contains the complete readable plan. Use headings, lists, tables, fenced code and file links; scale detail to the task. Ask consequential questions in the document and collect answers through plan feedback. Do not author HTML or claim/exhibit trees for new plans. The envelope retains immutable identity and native review/approval controls. Change the revision ID when content changes. Limits remain 1 MiB per document and 64 KiB per text field.

The conversation renders the complete Markdown. The right-panel **Plan** section lists the latest revision of each plan, including approved plans, as references for current work. Hover opens the same document in an anchored peek; crossing into it preserves the peek, click pins it, and Escape restores source focus. The inline document remains readable while its peek is open.

**Review response** previews exact canonical feedback. Explicit **Send response** sends it to the owning session; feedback never authorizes implementation. **Approve & implement** is a separate explicit action bound to the current immutable revision. Queues, delivery journals, duplicate protection and composer draft preservation remain unchanged.

Existing structured plans remain importable and convert to Markdown for reading. Legacy decisions retain native answer controls and explicit-answer approval gates. Their original stored documents are never rewritten.

Verification must name both **Codex and Claude**. Core tests establish import, persistence, revision and delivery contracts; native fixtures establish only the exercised UI behavior. Neither establishes live provider-boundary behavior. Inspect inline Markdown, latest-revision hover/pinning, source focus, narrow widths and zoom with real native rendering; report unavailable platforms and live checks explicitly.


## Integrated HTML reports

An agent can return a report that reads as part of the conversation rather than
a framed document. Ordinary HTML previews are unchanged.

### Contract

A complete HTML document (`<!doctype html>` or `<html>` through `</html>`, at
most 2 MiB) opts in with the exact comment `<!-- grove:integrated-report -->`.
It reaches the transcript the same way as any HTML preview: a closed fenced
`html` block, or a Markdown link to a local `.html` file.

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

- Inline, the report has no frame or title; a compact action row keeps Preview,
  Source, Reload (fenced reports), Expand and browser actions.
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
