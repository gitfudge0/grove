# Verification and known gaps

Commands run from the repository root unless noted. Historical records below
are context for investigation, never fresh test results or product requirements.

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

Thirteen focused snapshot-engine tests passed. The core run also passed all
four shared-skill parser/completion tests and seven commit-helper regressions. Coverage includes
staged/unstaged/untracked data without index mutation, bounded reads despite
concurrent file growth and final-component symlink swaps, omitted/binary/oversized
patches, immutable HEAD, cancellation of snapshot subprocesses, malformed model
output, GitHub pagination/truncated patches, and head/base/body guards on a fake
GitHub boundary. No real PR body was changed by verification.

The shared Codex/Claude composer fixture passed: both invocation forms route to
the native action without provider submission, preserve the command draft,
reject/preserve attachments, show invalid-argument usage, and avoid duplicate
built-in completions. Core startup fixtures also check both providers receive
the bundled skill text. This does not establish a live main-composer-to-provider
end-to-end path. Codex live generation on an isolated Git repository passed;
Claude's live attempt failed and `claude auth status` reported signed out.

Four Linux production-reader fixtures passed hidden/uninitialized Checkout skill
routing, publication-error draft preservation, file disclosure, captured-diff navigation,
Escape, stale publishing rejection, preview focus and source restoration.
Native reader captures exposed a fixed-height multiline-preview overflow; the
owning well now grows and a geometry regression checks input containment and a
reachable, nonoverlapping confirmation action. Captures exercise the reader in
an isolated native root, rather than the complete session/Checkout shell. They
are not participant testing. The final constrained narrow capture measured
320 logical pixels wide; the optional Mermaid capture retained an earlier diff
frame, so native Mermaid appearance remains unverified. macOS and Windows
remain untested.

Broad checks are not green. The initial workspace run aborted in
`activity_growth_keeps_heading_and_composer_anchored` after a stack overflow and
reported failures in approvals, measured message slots, task-strip navigation,
and a shell PTY fixture. The PTY and dashboard tests passed separately with a
16 MiB test stack; the three CodexView geometry/focus tests still failed in
isolation. The core run passed 817 tests with nine ignored and the two documented
GUI-environment failures above. A run excluding those two encountered the tmux
export-refresh test, which passed in isolation. These broader failures remain
unresolved. Strict core/UI Clippy also reports existing diagnostics outside the
new feature modules; the final reader and engine files have no reported lint
diagnostics. Formatting, compilation, and repository-artifact checks passed.

## Review changes

Linux focused evidence passed eight core review tests covering old/new changed-line
grounding, declared hunk bounds, unknown/omitted paths, duplicate or empty
coverage, scoped no-findings output, pre-call cancellation and literal Markdown
context fences. Four shared-skill parser/completion checks passed after correcting
a copied test query. A native test run passed 33 tests with three ignored;
subsequent coordinator checks passed the four final-reader tests (with the
native capture ignored), affected Describe regressions and source conformance.
Core review, skill-parser and shared snapshot checks were also rerun successfully. Coverage includes file disclosure,
captured-diff navigation/Escape, stale read-only results/source-focus restoration,
hidden Checkout routing and Files navigation yielding the canvas. Codex/Claude
composer fixtures verify command interception, preserved drafts, attachment
rejection, argument usage and duplicate-free completion. These are not a live
main-composer-to-provider end-to-end check.

Live Codex generation against disposable repositories passed coverage/freshness
and preserved HEAD, index and working state. An additional obvious authorization
regression fixture produced a grounded P1 finding at the changed line with a
concrete member-role trigger, deletion-permission consequence and corrective next
step. This is one observed model result, not a quality benchmark. Claude live
generation remains unverified: `claude auth status` reported signed out. No real
GitHub review or PR write was performed.

Linux native fixture captures were inspected for a result, expanded finding,
stale result constrained to 320 logical pixels, and a captured diff in light
appearance at rem 20. Normal readers measured about 570 logical pixels wide.
A separate single-state-process capture records the 320-pixel reader inside a
569.6-pixel native window (1424 physical pixels); it does not establish an outer
window width of 320. The back action remains visible. Native geometry tests at
320 logical pixels and rem 16/20 also verify its bounds after body scrolling and
captured-diff navigation. An earlier capture retained a previous frame; final
evidence records actual PNG dimensions and both window and reader bounds.
Sampled foreground/background contrast from captures: dark main 17.64:1, dark
supporting 8.18:1, light main 18.40:1 and light supporting 5.70:1. Native fixtures
use reduced-motion static endpoints. Complete session/Checkout shell flows,
animated transitions, live main-composer handoff, macOS and Windows remain
untested. These isolated fixtures are not participant testing.

New review generation resets the previous run's freshness-in-flight state so an
invalidated callback cannot permanently suppress checks after generation fails
or is canceled. The previous review remains available.

Formatting, compilation and artifact checks passed. Strict core/UI Clippy attempts
remain blocked by 21 inherited core diagnostics in unrelated code; no diagnostics
were reported in the new review/skill modules or changed feature lines. The UI
Clippy attempt did not reach the application crate because the core dependency
failed. Existing broad-suite failures documented under Describe changes remain
outside this feature's verification claim.

## Ask about changes

The Ask engine reuses the Describe snapshot collector and private tool-disabled
provider runner. Focused Linux core checks cover malformed structured answers,
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
