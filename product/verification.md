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

## Outstanding verification follow-ups

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
