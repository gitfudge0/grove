# Verification and known gaps

Commands run from the repository root unless noted. Historical records below
are context for investigation, never fresh test results or product requirements.

## Description action hover

Describe changes and View description use the shared checkout row padding and
full-card-width hover target. A GPUI regression checks the action edges against
its controls container and preserves the leading icon inset. Six description
interaction tests and all 33 UI conformance checks passed. Linux native controls
captures were inspected with a standard checkout row alongside the dark hovered
generation action and the light hovered saved-description action. The capture
fixture now drives frames and removes its window on exit; initial blank captures
were not used as visual evidence. Codex and Claude are verified through the
existing description interaction fixtures; live generation and macOS/Windows
hover rendering remain untested.

## Fixed Checkout group order

The grouped Checkout uses directory tools, Changes, Git, Pull request, Scripts,
then Session (Plan before Agents), omitting absent groups without urgency-based
reordering. Offline GPUI coverage passed 18 checkout tests, six description
interaction tests and two plan-preview tests; all 33 UI source conformance
checks passed. Codex and Claude are each **verified through fixtures** for the
changed layout and existing description/agent interactions, not live provider
calls. The Linux native full-Sidebar capture also passed after correcting its
frame scheduling and window teardown; light/clean and dark/working captures were
inspected for stable group order and saved-description placement.

Remaining gaps: the desktop window manager overrode requested narrow window
sizes to 1424 × 1698 logical pixels. Narrow-width and changed-rem assertions are
GPUI TestPlatform evidence, not verified narrow/zoom desktop captures. Live
Codex and Claude calls, real PR writes, exhaustive combinations of the mock's
states, and macOS/Windows rendering remain untested for this grouping change.
The capture fixture starts no provider, shell script or GitHub mutation.

## Provider size-recovery smoke check

The opt-in `grove-core` example exercises production plan import and retry through
an authenticated Codex or Claude CLI. Build it with
`cargo build --locked -p grove-core --example oversized_artifact_smoke`.
For each provider, create fresh disposable configuration and empty working
directories, then run:

```sh
GROVE_CONFIG_DIR=<disposable-config> cargo run --locked -p grove-core \
  --example oversized_artifact_smoke -- codex <empty-disposable-cwd>
```

Repeat with `claude` and new directories. It writes oversized fixtures, requests
literal links from the provider, and checks size rejection and retry after shrink.
It never submits approval responses. This is a live provider operation, not an
ordinary offline test; HTML rendering and native interactions are outside its
scope. Historical logs remain in `.grove/qa/`; the method and limits are recorded below.

## Provider contract and lifecycle verification

The provider-neutral contract extraction preserves serialized state, legacy
imports and recovery locations. Linux core verification passed 802 tests with
9 ignored, excluding two environment tests that also failed on untouched
`e0f999b`: `finder_path_resolves_standard_user_install_and_preserves_path_priority`
and `gh_uses_recovered_gui_environment`. An initial full run also failed the
activity listener shutdown-timing assertion; isolated baseline and the final run
passed it. The original intermittent failure is not resolved by this work.

New Claude regression fixtures failed before the fixes: pending tool rows were
settled before authoritative idle, and `terminal_reason: api_error` could report
Done with `is_error: false`. Fixtures now cover late failed tool output and
terminal failure. Fixtures do not establish live SDK behavior.

Codex live production-actor smoke on Linux verified command streaming, duplicate
native-thread refusal, persisted resume with remembered context, interruption,
and process/recovery cleanup. Claude live initialization and duplicate native-thread
refusal succeeded, but its first model turn failed with expired OAuth that could
not refresh; Claude command, resume and interrupt live checks remain untested.
Cross-process OS lock refusal and reacquisition passed for both provider namespaces.
The native steering eligibility fixture passed for both providers, and all 33
source conformance checks passed. This is fixture/source coverage, not a
native-widget-to-live-provider end-to-end check.

Both live transports emitted metadata-only diagnostic files; records were checked
for the exact fixed metadata keys, per-file size limit and Unix `0600` permissions.
Actual provider payloads are intentionally absent. Native screenshot/interaction
inspection is untested (native computer control is unavailable); macOS and
Windows are untested. The shared steering eligibility expression is preserved.

## Outstanding verification follow-ups

Skill and slash-command completion icons: the shared renderer omits the leading
icon and slot for both lists. Native appearance, hover/focus and selection checks
remain untested (native computer control is unavailable). Codex and Claude live
completion checks are untested; this change was checked at source/build level.


Persistent right-panel toolbar spacing: the toolbar moved down by `SPACE_3XL`
to sit closer above the card. Native visual checks at normal/narrow widths,
light/dark themes and changed zoom remain untested; native computer-use is
unavailable in this session. Codex and Claude live checks are untested for this
provider-independent geometry change. macOS and Windows remain untested.

Transcript minimap tick restoration: live Codex and Claude provider checks and
native visual/interaction inspection are untested for this glyph-only rollback.
The shared renderer restores the prior tick geometry; no installation was requested.

Session-header title/status spacing: Codex and Claude GPUI fixtures verified the
4px title-to-status gap, status-before-edit ordering, and existing title-edit
interactions on Linux. Live Codex and Claude provider boundaries are untested
for this layout change. Native screenshot review at narrow widths, light/dark
themes and changed zoom remains untested, as do macOS and Windows.

Sidebar icon spacing: destination and workspace-selector gaps now use `SPACE_LG`.
The 33 source conformance checks passed, but
`single_sidebar_destinations_grouping_and_collapsed_geometry` failed its
destination-right-edge assertion (152px versus 204px). Native visual inspection
at normal/narrow widths, zoom, and light/dark themes remains unverified.


The retired September sidebar restoration tracker recorded the following gaps.
These are historical verification debt, not current behavior requirements or
fresh pass/fail findings. Reconcile each with current code and `product/design.md` before
checking it; several sidebar recipes and terminal lifecycle rules have since
changed. The old macOS inspection attempts were blocked by a locked desktop or
computer-use timeouts. This cleanup does not resolve those checks.

- Navigation: grouping/collapse, selected-session identity, Sessions priority and
  ordering, sidebar drag/persistence/reset, narrow layouts and mode switches.
- Project removal: successful opt-in worktree deletion while preserving the main
  checkout was not exercised by the old editor test; cancellation, partial errors
  and file preservation require the relevant current workflow.
- Worktrees: guarded deletion/progress, backend availability and launch failures,
  repeated run-script sessions, collapsed activity and lifecycle/focus retention.
- Launcher: recents/search, keyboard/focus, narrow layout, multi-root launch context
  and actual launched-session identity.
- Session rows: title truncation, activity age, stale context-root fallback and
  live Git counters; native diff file/patch layout, refresh and return focus.
- Standalone terminals: live shell title, OSC7 cwd versus launch-directory fallback,
  exit status and final/nonfinal close behavior. Use the current explicit-open
  lifecycle, not the old tracker's replacement-shell expectation.
- Keyboard: scoped launch, next/previous/direct session selection and numbered
  badges, waiting-session jump, tree/list/grid switching, terminal creation/close,
  PTY Tab/Ctrl+C and modal focus routing.
- Settings: reachability, scrolling, nested navigation, persistence, disabled/error
  states, keyboard focus return and archived-project handoff.
- Cross-cutting: normal/narrow geometry, zoom, type, grid borders and terminal
  padding. Inspect real native views and report Codex and Claude separately;
  historical unit/fixture checks do not establish live provider behavior.


## Historical size-recovery evidence

Before connecting, the harness writes two harmless fixtures into its temporary cwd: valid schema-v1 plan JSON padded with whitespace to 1,048,726 bytes, and HTML padded with an inert comment above 2 MiB. The provider prompt requests only literal Markdown links without tools/file access. The completed actual provider response is processed by the production actor. Assertions require structured plan size metadata (exact bytes, 1,048,576-byte limit, canonical path) and HTML link in actual assistant items. The harness then shrinks the plan itself and sends production RetryPlanImport for the same message/reference. Assertions require error removal and successful import.

Historical results retained from the original run: Codex verified; Claude verified. Both processes exited 0. See the raw provider logs linked below. Relocation/build checks do not constitute a fresh live provider run.

Scope limit: this checks live provider-to-production-plan-import behavior plus actual HTML response link presence. It does not exercise the HTML renderer, automatic file watcher/UI refresh, native recovery-card appearance, browser opening, or Reveal file interaction. Those require separate native checks.

Raw logs: [Codex](../.grove/qa/oversized-live-codex.log) and [Claude](../.grove/qa/oversized-live-claude.log).

## Historical Activity reply follow-ups

The implementation note for commit `325a778` reported Linux native inspection,
GPUI fixtures for Codex and Claude, and live Codex actor delivery. It explicitly
left live Claude delivery untested because OAuth refresh failed, and left macOS
and Windows untested. Fixtures did not establish native-widget-to-live-provider
end-to-end behavior. Per-element backdrop blur was unsupported; no measured
reference certification was claimed. Reverify these boundaries when touched.

The broad composer run reported 42 passes and a baseline-reproduced failure in
`task_strip_docks_above_composer_toggles_and_jumps_to_task_start`. Strict Clippy and
formatting also had recorded baseline failures. These are historical reports,
not fresh results or authorization to change unrelated code.

## Historical checkout-terminal follow-ups

The implementation note for `be4a2bd01feba7362c8333cb88be4ce157de9cf5`
(starting revision `82ce9b689a900ecf6cc4ef35d3bdbe48b7102232`) reported
1,105 passes, 11 failures independently reproduced on its starting revision,
and 7 ignored tests. Focused Checkout/Home/pane checks and a real linked-worktree
shell integration passed in that run. Linux/XWayland native inspection observed
`pwd` in the checkout-owned PTY; light expanded-pane and raster motion states
remained unverified. Home Activity/composer overlap was also reported on its
starting revision. Both providers had draft-preservation fixtures, but live
Codex and Claude boundaries were untested; macOS and Windows were untested.

These summaries preserve useful limitations from retired scratch notes. Their
screenshots and baseline logs are not current acceptance evidence. Run the
relevant checks against the actual checkout before claiming any gap resolved.

## Composer code font follow-up

The composer code-font repair applies JetBrains Mono to inline and fenced code
and measures wrapping with those font spans; code decorations no longer draw
fragmented outlines around each source line. Native visual inspection at normal
and narrow widths, light/dark themes and rem zoom remains outstanding on Linux,
macOS and Windows. Codex and Claude share this composer path; fixture checks
do not establish live provider delivery or native visual appearance.

## Review summary chevron alignment (November 2026)

The disclosure chevron now centers within the first body-text line box rather
than the top of the multiline block. Focused review-summary tests passed,
including expand/collapse and narrow-width/zoom fixture interactions. Codex and
Claude completion selection and link parsing were verified with fixtures only;
live provider paths remain untested. Native screenshot inspection of alignment
in light/dark themes and at changed zoom remains untested on Linux, macOS and
Windows.

## Activity category review settling

The Review category retains pending reviewed rows while the pointer remains
inside it, including sibling rows and gaps; leaving starts a fresh 400 ms delay.
Linux GPUI timer-state checks cover category entry/return cancellation, the
400 ms endpoint, keyboard focus gating, and new work bypassing the hold.
These invoke the shared state handlers, not real pointer events or providers.
Codex: live provider path untested. Claude: live provider path untested.
Native category pointer crossing and visual inspection remain untested;
macOS and Windows remain untested. The environment has no enabled native
computer-use surface for this interaction check.

## Sidebar activity-group settling and motion

Activity-group placement now waits for a stable 500 ms destination, with immediate
Needs you entry and live status marks. Linux pure state checks cover burst
coalescing, cancellation and the exact settling boundary. Linux GPUI motion
fixtures exercise Codex and Claude session metadata, passive departing rows,
destination entrances, lifecycle closure and reduced-motion endpoints. These
are fixture checks, not live provider-boundary verification: Codex live is
untested; Claude live is untested. Native visual inspection, real pointer and
keyboard focus across moves, light/dark themes, narrow widths and zoom remain
untested; macOS and Windows remain untested. No enabled native computer-use
surface is available for those visual checks in this session.

## Checkout push motion

The option B rising-arrow push indicator has a Linux native capture fixture (`push_animation_linux_capture`). The initial X11 capture produced blank images, so native appearance, motion continuity, narrow layout, light/dark contrast and reduced-motion rendering remain unverified. Real remote push success/failure was not exercised. macOS and Windows remain untested.

Push completion confirmation: option C uses a 440 ms check draw followed by a three-second receipt. Timing, stale expiry and checkout ownership have focused offline tests. Native draw appearance/continuity and reduced-motion rendering remain unverified after the earlier blank X11 captures; real remote success/failure, macOS and Windows are untested. This change was explicitly delivered without installation.

## Side-panel action feedback acceptance

The side panel now shares rotating working marks and 440 ms drawn completion checks followed by a three-second hold. The lifecycle is owned by checkout/action identities and script run IDs; failures do not create completion receipts. New Git work supersedes previous Git receipts. Create PR is a browser-form handoff, unlike the current-chat prompt and CI queues. Script stopping requests termination and waits for observed PTY exit before its receipt/removal.

Linux offline evidence: four shared receipt tests cover owner isolation, stale completion/expiry, reading deadlines and retained-row collapse. Thirteen terminal tests, including a real PTY stop/observed-exit check, 18 checkout tests, 10 local Git regressions, the script-cwd regression and all 33 source conformance checks passed. Native GPUI test fixtures cover fixed icon/label slots, narrow/default widths and inert completed controls. Codex and Claude are verified only through transport-channel fixtures for named prompt queueing/draft preservation and fresh resolution drafts; neither is a live provider-boundary check. Current Linux X11 captures of the shared native action rows were inspected in Grove Dark and Grove Light at rem 16 and in reduced-motion Grove Dark at rem 20. Early/later native frames show ring rotation and check-stroke reveal; reduced-motion frames are identical. The native fixture uses a long branch label, and GPUI layout/input fixtures cover 280/180 logical-pixel widths and disabled activation. Sampled working/success foreground-to-card contrast is dark working: 7.60:1; dark success: 9.92:1; light working: 6.16:1; light success: 6.14:1. Full end-to-end native flows for every action, live Codex and Claude CI/prompt handoffs, real GitHub PR operations, macOS and Windows remain untested. The requested delivery endpoint is commit only, with no installation.

## Browser link restoration (Linux, commit-only)

Browser-labeled local links again launch the default browser; the file viewer
also provides a centered browser action for HTML, PDF, SVG and common raster
images. Focused browser/file-viewer checks passed 14 tests, including native
GPUI layout bounds at 240/500 logical pixels and rem 16/20. All 33 source
conformance checks passed. Codex and Claude ordinary local-file viewer behavior
passed through the shared GPUI transport fixture; live provider-boundary behavior
is untested for both. Actual browser process launch, native screenshot/state QA,
macOS and Windows remain untested. No installation was performed.

The combined working tree's core run passed 801 tests with 9 ignored and failed
three: `rejects_browser_duplicate_and_oversized_requests`,
`finder_path_resolves_standard_user_install_and_preserves_path_priority`, and
`gh_uses_recovered_gui_environment`. The latter two were already documented
above; this run does not establish the cause of the activity test failure.

## Describe changes

The panel-controls follow-up passed ten focused native reader/lifecycle fixtures,
including source-menu pointer/keyboard selection, Enter reopening, Escape/focus
restoration, single-source visibility, loading rejection and PR-only fallback.
Saved-page navigation fixtures cover both Codex and Claude; live provider calls
were not repeated for this UI change. Linux production-control captures were
inspected for local generation, source selection, saved navigation and staleness
in dark and light themes. The changed-rem capture did not produce usable content,
so zoom appearance remains unverified; this does not close the full-shell light
rendering gap recorded below. The 33 UI conformance checks also passed.

The current workflow creates a retained provider conversation and stores one
canonical result per PR or local checkout. Focused engine tests cover immutable
capture, cancellation, malformed/unknown/duplicate results, bounded conversation
imports, omission coverage, PR pagination and head/base/body guards. The eight-word
summary target is editorial; nine-word bullets remain valid, while the existing
one-to-four bullet, character, type, path and diagram limits remain enforced.
PR freshness can be checked from another checkout; publishing still requires the
capture checkout or regeneration there.

The delivery run passed 29 combined engine/storage tests, five command-parser
tests, the independent-conversation preference test, nine native reader/lifecycle
tests, the conversation-presentation test and the shared Codex/Claude composer
fixture. Opt-in native captures and live-provider acceptance are ignored by
ordinary test runs and were exercised separately below.

Storage checks exercise restart reload, stable source paths, PR normalization,
local-root isolation, private files, corruption preservation, pending capture
recovery, unique generation attempts, old-request rejection, terminal retirement,
and canonical-result publication guards. Imports compare their own generation
token under a cross-process source lock. A superseded or canceled request cannot
replace the canonical result, delete a newer job or be revived after retirement.

Codex and Claude native fixtures exercise the real linked Describe changes panel
action even without local edits or an attached PR, Conversation/Escape return,
focus and draft/image preservation, stale publishing rejection and session
isolation. Request-matching fixtures cover repeated generations, restart,
incomplete output and a completed captured turn followed by an active or failed
unrelated turn. Fresh-session checks preserve provider/model preferences while
clearing conversation ownership and retaining the originating draft.

A live normal Codex conversation generated and persisted a local description in
an isolated configuration; reopening the store returned the same canonical result,
and fixture HEAD, index and working state stayed unchanged. The initial default
model was unavailable to this account; an isolated supported-model override passed
without changing global configuration. A normal PR #17 conversation returned a
20-file result but exposed the former eight-word hard rejection. After the bounded
validator fix, the exact retained response was imported against its original
pending snapshot and persisted/reloaded successfully, without another model call.
The supported smoke is `describe_session_smoke`, with an empty temporary
`GROVE_CONFIG_DIR` and optional `GROVE_DESCRIPTION_SMOKE_MODEL` override. Claude's
auth check reported signed out; no live Claude generation ran. No PR body was
published by these checks.

A live native Linux acceptance also passed from `Sidebar.invoke_description_skill`:
it created a distinct Codex session/key, preserved the source draft, captured the
local fixture, completed the normal provider turn, imported and saved the result,
reloaded the same canonical result/link, cleared its pending job, returned to the
new conversation and reopened the same reader with native right-panel Enter.
Changing the isolated fixture produced staleness while preserving the saved data.
The owned provider's shutdown acknowledgment was required for success. This starts
at the native skill handler; the composer Submit boundary is fixture-tested
separately. An initial acceptance attempt reached successful generation/import but
looked in the wrong test configuration: SettingsState's test isolation replaced
GROVE_CONFIG_DIR. The maintained harness now requires matching empty
GROVE_CONFIG_DIR and GROVE_TEST_CONFIG_DIR and checks the resolved path before
starting. The corrected run passed without writes to user configuration.

Linux native full-shell QA uses an isolated configuration and mocked session
connection. It observes the actual Sidebar, Checkout card, conversation, linked
button and native reader. This is distinct from the live core-provider smoke;
the live acceptance below separately exercises the normal provider. Actual
Codex mock shell captures at 569.6 by 679.2 and 1424 by 1698 logical pixels passed
native Enter/Escape navigation, retained drafts, reader/panel coexistence and stale
state. Capture found and corrected a narrow header/toolbar overlap and a linked
action below the card's visible scroll area; the action now leads the card and
the header reserves the toolbar's shared width. Narrow and wide dark recaptures
were inspected after the fixes. The requested light candidate still captured a
dark frame despite Light metadata, so light appearance remains unverified.
Native captures require a 16 MiB test stack in this environment; the default-stack
attempt overflowed. The initial window measured about 570 by 679 logical pixels
due to X11 scale; capture metadata records actual dimensions rather than treating
requested window size as evidence. macOS, Windows and optional Mermaid appearance
remain unverified for this revision.

The full workspace and strict Clippy suites were not rerun for this correction.
Earlier broad checks had unresolved approvals, measured-message/task-strip and
GUI-environment failures; those are not established as fixed by focused feature
checks. Formatting, compilation and repository-artifact policy remain delivery
checks; focused suites do not imply a green full workspace.

Transcript file-link explorer visibility: the shared sidebar routes preserve the
user's explorer and side-panel choices while opening the file viewer. The Linux
GPUI regression exercises Codex and Claude session fixtures and the native
browser-to-transcript event chain with the explorer hidden and shown. Live Codex
and Claude provider-boundary checks, native pointer/keyboard and screenshot QA,
and macOS/Windows checks remain untested for this change.

## Built-in prompt customization and richer descriptions

Settings → Prompts provides saved writing overrides for commit messages, PR text,
Describe changes and session titles. Offline evidence includes prompt storage and
fallback tests, legacy description loading, Markdown/export validation, repeated
PR section replacement, the Settings save/restore/draft-preservation fixture, and
all 33 UI conformance checks. The Settings fixture also checks a large rendered
editor height. Linux native Settings captures were inspected in Grove Light and
Grove Dark, including changed rem sizing. The desktop window manager overrides
requested window sizes; these captures do not establish narrow-window behavior.

Live Codex checks passed saved custom commit and PR structured writing, plus a
normal retained Describe changes conversation requesting an exact heading and
table. The description was validated, saved and reloaded with the captured source
still current; fixture HEAD, index and working state remained unchanged. The
structured-writing smoke does not commit or exercise the real GitHub compare/form
handoff. Live session-title customization and real GitHub publication remain
untested. Claude's normal-session check failed because its OAuth session expired
and could not refresh; its isolated structured-writing check also failed. Live
Claude customization is therefore unverified, despite shared offline coverage.

The broad core run passed 842 tests, failed three and ignored nine. Sequential
reruns still failed the executable-resolution and recovered-GitHub-environment
fixtures because installed Codex/GitHub executables were selected instead of their
fixture executables. The tmux startup-environment check passed in isolation. These
failures were retained rather than changing unrelated executable discovery.
macOS and Windows execution and exhaustive native hover/focus/error states remain
untested. Long title instructions are transported on stdin to avoid oversized
Windows command-line arguments; command-shaping tests do not establish Windows
runtime behavior.

The native description fixture passed after synchronizing captures with actual
platform frames. Inspected custom dark/wide and light/500-pixel-reader/rem-20
captures show the requested table and highlighted code, with file walkthrough
and coverage retained. This establishes the constrained native reader fixture,
not a full narrow desktop-window interaction. The title argument regression
suite passed 18 tests (four live tests ignored); the final enlarged-editor test
and conformance rerun also passed. The real PR comparison flow may safely refuse
comparisons beyond GitHub's default 250-commit page; no real GitHub PR form or
publication was exercised in this run.

## Prompt Settings compact picker (2026-11-06)

The selected variant A replaced the four-editor stack with a two-by-two picker,
one full-width native editor, independent draft status, byte count and a collapsed
read-only requirements disclosure. Save and Restore stay in the shared action
row. The automatic-save notice is excluded from this explicit-save category.
The existing generation, storage and provider contracts were not changed by this
presentation pass.

All 19 Settings tests and 33 UI conformance checks passed. GPUI input/layout
fixtures exercised independent draft preservation across prompt/category switches,
Space typing, Tab/Enter selection, a focused Space disclosure action, reduced-motion
expanded/collapsed geometry, empty-save rejection, Save/Restore persistence, and
480px layout at rem 16/24. These are synthetic input fixtures, not OS-level pointer
or screen-reader testing. Native Linux X11 captures were inspected for Grove Dark
and Light, default/unsaved state, the long session-title prompt, expanded contracts,
and a constrained 480px content area at rem 16/24. The desktop window remained
larger than the constrained content area. Eight final isolated capture invocations
passed; an initial requirements launch stalled and passed on retry. Earlier
cross-theme capture lag and input-handler teardown failures were resolved by
capturing one state per app and flushing input ownership before shutdown.
Evidence: `.grove-artifacts/prompt-settings-design-v1/` (Settings/conformance logs,
`native-v6/900`, `native-v6/480`, and the requirements retry log). Supporting-text
contrast from the native palette is 8.18:1 dark / 5.70:1 light; editor text is
16.09:1 dark / 16.00:1 light. This does not establish full accessibility conformance.

The exploratory HTML remains a layout direction, not a measured native reference:
the reference author command reports native capture unsupported on Linux, and the
current typed vocabulary does not cover this wrapping editable form. Certified
reference parity, macOS/Windows runtime, OS-level hover/press/release inspection,
and animated interruption/exit inspection remain untested. Codex and Claude live
provider-boundary behavior was not re-run for this Settings-only presentation
change; both remain untested in this pass, with prior generation evidence and
Claude authentication limitations recorded above. Formatting, whitespace and
repository artifact checks passed before installation.

The final `./install.sh` completed successfully; the installed `~/.local/bin/grove`
matched `target/local/grove` byte-for-byte. Delivery stayed on `feat/new-sidebar`
without a commit, push, PR or release.


## Retained Review parity

Review uses the shared retained analysis controller and a private typed store.
Eleven core Review tests passed: existing finding grounding/coverage, strict
conversation import, canonical restart reload, and derived-cache reconstruction.
Only authoritative fields are persisted; legacy derived file text is ignored and
rebuilt through the validator. Five command parser/completion checks passed after
making a skill-text assertion independent of Markdown line wrapping.

Seven focused feature tests passed, covering native source-picker keyboard and
disabled behavior, captured diff/Escape, stale results, narrow back bounds, a
both-provider channel fixture for retained result recovery/permanent View and
same Conversation, factory model/draft preservation, and linked external PR
routing without a registered checkout. The recovery fixture uses a fresh
controller, explicit Idle/result-none wait, observed Sidebar updates and a
refreshed frame. It does not treat an old in-memory cached result as a restart.
Missing initial results expose explicit generation without automatic resubmission.
Dashboard/normal-composer routing was also exercised before catalog readiness.
Nine prompt/settings checks, four shared controller checks and 33 source
conformance checks passed. Production compilation and formatting passed. Strict
core/UI Clippy remains red on inherited diagnostics; the final logs contain no
Review, review-skill or newly shared-controller/matcher diagnostic references.

A normal Codex conversation in an isolated disposable Grove configuration passed
with a persisted custom Review prompt. It returned a requested Markdown heading
and table plus a grounded P1 authorization-regression finding, saved/reloaded the
canonical result, retired its pending request, remained current and preserved
HEAD, index and working state. Its owned provider process acknowledged shutdown.
This was normal transport, not the old isolated one-shot helper. That first run
used the backend default model without recording its resolved identity; future
smoke output records model and resolved_model. Claude authentication was checked
again and reported signed out, so live Claude remains unverified. No real GitHub
review or PR write was performed.

Linux production-widget captures cover Local/both sources, selected source,
saved/stale controls and light/zoom variants; inspected controls were readable
without overlap. The menu raster did not display choices and does not establish
popup visual correctness; keyboard selection has separate native test evidence.
Reader captures use frozen validated-shape fixture content rather than a complete
main-composer/provider handoff. Native full-shell flows, animated lifecycle,
live main-composer end-to-end, macOS and Windows remain untested. A constrained
320-logical-pixel reader is distinct from the wider outer native window, as
recorded in capture metadata.
review or PR write was performed. Full application shell flows, macOS and Windows
remain untested until separately observed.
## Ask about changes

The initial Ask implementation reused the Describe snapshot collector and a
private tool-disabled provider runner. The retained-session update below replaces
that UI generation path with normal provider turns. Focused Linux core checks cover malformed structured answers,
unknown/omitted/duplicate examined files, citations outside captured old/new hunk
lines, deletion-side references, cross-hunk ranges, cancellation before provider
launch, and empty questions. Snapshot/no-index-write guarantees retain the
existing Describe regressions. Both-provider composer fixtures check native
`/ask`/`$ask` routing, reserved completions, command draft preservation and
attachment rejection without provider submission. These fixtures do not prove a
live main-composer-to-provider end-to-end path.

A live Codex Ask call on an isolated Git fixture produced a validated answer
citing new lines 2–5 of the captured patch; Git status and the index were unchanged.
The live Claude attempt failed; `claude auth status` reported `loggedIn: false`
and `authMethod: none`. Claude live generation remains unverified pending sign-in.

Native production-reader fixtures exercise captured-reference navigation,
Escape/source focus, stale/error answer preservation, question-only opening,
session/checkout/scope ownership and deferred-submit checkout isolation. Native
captures use the production reader in an isolated Linux root, not the full Grove
shell or a live provider session. macOS and Windows remain untested. Existing
broader suite and strict Clippy failures recorded above are not claimed resolved.

Final focused results: five Ask core tests, two skill-parser tests, nine Ask
native/keyboard tests, one both-provider composer test and four inherited
Describe reader regressions passed. The keyboard fixture caught the vendor
Input consuming Control+Enter before raw key handlers; the reader now captures
its native secondary-Enter action, preserving plain Enter as a newline.
Formatting, compilation and repository-artifact checks passed. Strict core
Clippy still reports 21 diagnostics outside Ask; strict UI-only Clippy reports
103 outside the new Ask modules after correcting its new diagnostic.

Linux native answer captures were inspected in Grove Dark at rem 16 and Grove
Light at rem 20, plus a captured-diff view, provider-error state and a 340-logical-
pixel constrained stale reader. An initial capture quit with an InputState leak;
the harness now focuses its source and releases the reader before quitting.
One-state-per-process captures avoid retained frames; the corrected narrow
bootstrap capture shows the actual wrapped reader. These remain isolated native
fixtures, not full-shell interaction, live provider UI, or participant testing.


## Retained Ask conversations and durable answers

Ask now uses the shared typed result store/controller on normal retained provider
sessions. Eight Ask core tests cover its schema/citations, fenced conversation
import, exact question identity, canonical replacement and immutable prompt
requirements; three parser tests cover local/attached/explicit PR URL forms.
Both-provider fixtures passed two actual submit/router tests (including blocked
Dashboard catalog discovery), typed recovery/no-resubmission and ordinary-followup
isolation in the fourteen native Ask tests (one capture test remains explicitly
ignored by the normal test run). Final cases also cover same-runtime saved-key
replacement, linked explicit-source defaults and fixed-header bounds after
input focus/scroll at 340 and 900 logical pixels, rem 16 and 20. Three prompt-core tests, three Settings
prompt tests and four shared controller tests passed. These are fixture checks,
not live main-composer/provider UI checks. The initially reused shared-target core
binary ran zero Ask tests and was rejected; touching the package entrypoint forced
an actual current-source build and the expected eight test names/counts.

An isolated live normal Codex conversation passed with a persisted custom Ask
prompt: the answer included the requested Markdown heading/table and validated
captured references. The exact multiline question and composed prompt survived
pending storage; canonical result/link reload and pending cleanup passed, with
HEAD, index and working state unchanged. This exercised the normal provider
transport, rather than the old one-shot helper. A fresh Claude auth check reported
signed out, and the normal Claude smoke failed with expired OAuth credentials.
Claude live generation remains unverified pending sign-in.

The initial request asks the provider to avoid tools/tests/writes, but normal
session permissions apply. The canonical footer therefore says test results were
not verified, rather than asserting that no test process could have run. Native
captures are isolated production-reader fixtures; full-shell source-selector and
provider UI behavior, macOS and Windows remain untested. Existing broad-suite
and strict Clippy baseline failures are not claimed resolved. Coordinator final
checks own the current integrated revisions and final installation.

Final reader captures are under `.grove-artifacts/describe-parity-ask/`. The
`final-v2/stale-narrow.png` frame omitted the header despite passing native bounds
checks; it is preserved as unsuccessful visual evidence. A fresh single-state
`narrow-probe/stale-narrow.png` from the same immutable binary shows the complete
Conversation header, Ask heading and wrapped stale answer. This establishes that
the earlier raster was transient capture/frame evidence, not a reproduced
production geometry defect; full-shell narrow behavior remains unverified.
