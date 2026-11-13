# Verification and known gaps

## Ten user/agent transcript turns (November 2026)

The default history window and reveal batch are ten turns, counting each user
message and the following agent response separately. Commentary and tool activity
stay together within the agent response. Linux checks passed 40 history-filtered
tests, the full-history measured-slot geometry/focus fixture, 33 UI conformance
checks, formatting and repository artifact policy. The shared history fixture
exercised Codex and Claude rendering, pointer/keyboard reveal and collapse,
minimap navigation, browse preservation, narrow widths and changed rem size.
Full-history layout fixtures explicitly expand after session initialization.

**Codex and Claude:** verified through synthetic GPUI fixtures only; live provider
transports and manual native appearance inspection remain untested. No
platform-specific behavior changed; macOS and Windows were not exercised.

## Composer startup work reuse (November 2026)

The Dashboard retains worktree rows, reuses matching results for ten seconds and
joins an existing refresh. Retry and worktree creation force discovery. Draft
commands wake the UI after enqueue instead of polling every 40ms. Provider input
scans are shared per scope for five seconds, fingerprint only the selected
provider's skills and carry their model key through discovery. Model lists expire
after 24 hours; warm remembered rows stay visible during refresh. Claude shares
its permission-help probe across matching executable/environment identities.

Linux checks passed 27 focused core cache/probe/wake tests, 66 Dashboard fixtures,
seven model-picker fixtures and 33 UI conformance tests. Dashboard fixtures cover
both Codex and Claude draft wake/coalescing, handoff, draft preservation, worktree
reuse/retry and returning after a workspace-discarded Git result. The new actual
Git regression initially assumed a parked scheduler meant subprocess completion;
it now awaits bounded completion notifications and follows real project navigation.
Two previously documented header-inset tests remain excluded; five Dashboard and
one picker opt-in fixtures were ignored in the ordinary filtered runs.

**Codex and Claude:** live metadata-only discovery passed cold/warm lookup,
model-only reuse in a fresh process and explicit refresh, using disposable Grove
configuration and empty checkouts. Diagnostic handshake counts were one for each
cold/warm process, zero for restarted model-only lookup and one for forced refresh.
These checks sent no model prompt and created no conversation state. Probe
coalescing, expiry and clock rollback were exercised with local automated tests;
24-hour expiry was not observed by waiting a day against a live provider.

Four fresh isolated Linux X11 captures were inspected: Codex and Claude Ready,
Codex expanded preparation (activated with native Space without altering the
draft), and Claude narrow Light error at changed rem zoom. The rendering owners
were unchanged by subsequent time-unit/lint and request-completion test repairs.
This is component inspection with synthetic sessions, not a live end-to-end
composer/send test. macOS/Windows runtime, OS IME/screen-reader interaction and
comparative CPU/latency benchmarks remain untested. No local installation,
user-app restart or release was performed.

Filesystem notifications, a single subscription-driven watcher service and full
composer-metadata persistence remain deferred. Per-session five-second watcher
timers remain, but matching scopes share their expensive input scans. On a cold
restart, expired disk model rows are not published before discovery completes.

## Main page composer preparation

The Dashboard composer shares worktree discovery, provider options, image and
attachment preparation, and first-message handoff progress in one status slot.
Codex and Claude both passed GPUI fixtures for overlap disclosure, stable idle/
loading geometry, draft and image ownership, worktree retry routing, blocked
keyboard submission, native-command Send access during catalog loading/errors,
and focus restoration when status controls disappear. The real Dashboard
replacement/adoption path is covered with synthetic session actors; observation
uses weak references and does not own provider lifetime.

The scoped Dashboard run passed 63 tests with five opt-in fixtures ignored and
two explicit baseline exclusions. Those two header-inset fixtures
(`dashboard_position_uses_its_own_inset_viewport` and
`dashboard_adoption_keeps_composer_anchored_on_first_frame`) also failed on a
fresh build of the captured base: they expect a viewport without the existing
48px header. The startup Ready-layout fixture now settles worktree preparation
before checking its anchor. A parallel catalog-generation failure was traced to
first-time test-config initialization changing the environment fingerprint;
that fixture now initializes the shared config before taking either fingerprint,
and the complete scoped rerun passed.

Linux X11 QA used a separate app identity and configuration with synthetic Codex
and Claude sessions. Ten fresh-process cases passed and were visually inspected:
Ready, overlapping preparation, expanded disclosure, long provider-option error,
and connection handoff for each provider. Native synthetic Space activated the
disclosure without changing the draft. Dark/Light appearance and a constrained
360px composer at 20px rem were exercised; narrow model summaries retain their
full text by wrapping within the Dashboard toolbar. Both narrow error captures
were rerun after that correction. Desktop tiling limited the ordinary capture
viewport to 701px logical width rather than the requested 900px.

The 33 UI conformance checks, eight reply-suggestion regressions, strict Clippy,
formatting, reference comparator and repository/tooling checks passed. Early
shared-cache zero-test results were discarded. Initial native capture attempts
had stale compositor frames and a fixture input-handle teardown leak; fresh
processes and explicit empty-root teardown corrected the harness and reruns
passed. These are component captures, not full Home-screen or live transport
checks. Live Codex and Claude prompt delivery, macOS/Windows rendering, native
pointer/retry interaction and animated spinner appearance remain untested;
reduced-motion marks were captured. No app installation, restart or release was
performed.


## Provider catalog reuse

Linux metadata-only checks used Codex CLI 0.167.2 and Claude Code 2.1.323 with
separate disposable Grove configuration, checkout and diagnostic directories.
Both providers passed cold discovery, warm lookup, model-only reuse in a fresh
process, reuse from another empty checkout, and explicit refresh. Diagnostic
subprocess-file counts were respectively 1, 0, 0 and 1 for cold, restarted,
other-checkout and forced-refresh runs. Codex advertised eight models and Claude
five. These were metadata handshakes, not model turns or conversation/resume tests;
no end-to-end prompt-delivery claim follows from them.

The core suite passed 923 tests with ten opt-in checks ignored and one existing
tmux test excluded. The excluded immediate server-recreation test failed during
the unfiltered run and reproduced on unchanged main on its third focused attempt.
Six picker fixtures and 13 deferred-session fixtures passed, including provider
scope, retained rows, stale replies, keyboard selection and draft preservation.
The cache-split provider-switch fixture seeds memory only and has a rejecting
launcher fallback, so a cache regression fails without launching installed CLIs.

Native Linux Dark captures showed seeded Ready, Refreshing and retained-model
failure states for both providers. A native Space keystroke activated Refresh
without touching the other provider. The cleanup leak in the initial QA fixture
was corrected and the rerun passed. Requested wide/narrow window bounds were
replaced by desktop sizing (2880×1800 and 1402×1708 physical captures), so the
intended 360px narrow window remains unverified. Native Light and macOS/Windows
rendering and provider/keychain behavior remain untested. The 33 UI conformance
checks and all-target strict Clippy passed. No application installation or restart
was performed.


## Platform-specific window controls

Windows/Linux controls occupy the far right of the whole window; the content
canvas starts below the full-width header so pane actions do not overlap.
macOS retains leading traffic lights and fullscreen behavior. Policy coverage
checks all three OS cases, Linux decoration ownership and advertised controls.
The 44 shell tests passed, including window-edge bounds at 1280/500/320px,
1x/1.5x zoom, fullscreen transitions, sidebar collapse and keyboard traversal.
The 33 conformance checks, seven icon tests and two XWayland decoration tests
passed. Removing XWayland support in an isolated mutation made its regression
fail. Formatting, repository-artifact policy and the optimized Linux build passed.

An isolated production Linux X11/XWayland app on Omarchy was visually inspected
in Grove Dark/Light, normal/narrow windows, long workspace names and 1.5x app
zoom. Controls remained at the window edge above the content toolbar. Rendered
resting glyph/background contrast was 7.78:1 dark and 5.32:1 light. A real X11
fullscreen request round-tripped. A synthetic state-only X11 maximize property
change switched the square to the restore glyph without resizing. Omarchy
ignored the EWMH maximize request; this is not a maximize-action pass. Portal
callback startup no longer reproduced the RefCell panic observed before the
borrow-release correction.

Native pointer injection did not reliably activate the controls. Native
click/hover/press, keyboard activation, minimize, close cleanup and drag/double-
click separation therefore remain unverified; GPUI keyboard fixtures and policy
tests do not replace that evidence. macOS and Windows native behavior is untested
on the Linux host. Direct Wayland is unsupported by Grove's X11-only Linux
bootstrap; traditional X11 server decorations were covered by policy tests only.
Codex and Claude shared session-header GPUI fixtures both passed. Live provider
boundaries are untested for both; provider transport behavior did not change.

The baseline Clippy style warnings in these paths and their window-control
fixtures were corrected mechanically during provider catalog delivery. No lint
rule was weakened.

## OS display scaling

Linux X11 startup selects an explicit `GPUI_X11_SCALE_FACTOR` first (including
`randr` mode), then valid `GDK_SCALE`, `Xft.dpi / 96`, physical RandR DPI, or 1x.
Eight selection regressions cover precedence, malformed automatic inputs,
fractional Xft DPI, explicit override errors and fallback. An isolated mutation
removing GDK selection made the original mismatch test fail at 2.5 versus 2.
The existing three X11 file-drop regressions and 33 UI conformance checks passed.

A disposable Linux GPUI probe used a separate app ID, the production native HTML
bridge, and no provider services or user configuration. On this desktop it logged
`GDK_SCALE: 2`; X11 image pixels measured matching native and HTML 100-logical-pixel
markers at 200 physical pixels, then 240 pixels with 1.2 app/rem and HTML zoom.
Additional environment-controlled runs measured 100/120 pixels at 1x and 125/150
pixels with an explicit 1.25 override. These exercise the renderer/bridge at
fractional scale, not a fractional desktop/compositor configuration. Host window
physical dimensions also matched the reported logical viewport times its scale.
No HTML bridge change was needed.

macOS keeps AppKit logical points, native backing-scale notifications and WKWebView
hosting. Windows keeps GPUI's default PerMonitorV2 manifest, effective monitor/window
DPI, `WM_DPICHANGED`, and WebView2 logical bounds with physical clipping. Those
pinned-source boundaries were inspected; neither platform's runtime or cross-monitor
behavior was tested on the Linux host. The X11 fix does not enter those backends.

Full-shell terminal/composer/menu inspection, pointer targeting, scrolling and
narrow-window clipping remain unverified for this patch. Codex and Claude transcript
surfaces and live transports were not exercised. The existing native HTML integration
script requires unavailable Xvfb tooling; the probe above does not replace that full
interaction suite. XSettings-only desktops, live Linux scale changes, mixed-DPI
monitor moves and compositor-side double-scaling configurations remain outside
this startup fix. Do not infer those outcomes from parser tests or marker pixels.

## Fuzzy composer completion

File, skill and slash-command completion share the existing launcher fuzzy matcher.
Codex and Claude catalog fixtures cover case-insensitive subsequences, ranking,
disabled skill exclusion, host-command deduplication and the skill result limit.
Live Codex and Claude catalogs and native popup/keyboard interaction for fuzzy
queries remain untested; source fixtures do not establish provider transport or
rendered behavior. No app installation or restart is part of this change.

## Ambient composer focus glow

Spread transitions now use the shared slow interval (220 ms) and easing in both
directions, retargeting from the current value on interruption. Two focused
state tests passed for intermediate values, reversal continuity, unchanged-target
redraws, exact settled endpoints and reduced-motion interruption. The Linux
Codex/Claude synthetic-transport native capture exercised focus/blur and inspected
settled endpoints; frame-by-frame native transition appearance, live provider
transport and macOS/Windows motion remain untested. Composer layout, compact
reply draft/keyboard behavior and all 33 conformance checks passed.


Follow-up: tightened the shadow footprint and added a ten-second opacity pulse
while focused. Blur retains a tighter, steady low-intensity glow; disabled and
collapsed inputs hide it. The effect is restricted to the exact Grove Dark and Grove Light themes (including exclusion
of custom copies). Theme-catalog restriction, composer edge alignment, compact
reply keyboard/draft/acknowledgment and 33 conformance checks passed. Linux native
fixtures exercised Codex and Claude with synthetic transports: reduced-motion
steady focus/toolbar blur in both Grove themes, Tokyo Night without glow, and
16/20px rem; a Dashboard composer fixture also captured a complete pulse with
focus and draft preservation. This Dashboard fixture includes the destination
tray, not the complete Home layout. Desktop tiling constrained captures to about
570px wide. Live Codex/Claude transport, native compact-reply pulse, full Home
placement, wide native rendering and macOS/Windows visuals remain untested.
No installation or restart was performed for this follow-up.

Previous implementation evidence (before the pulse follow-up):

Option B uses a shared, theme-owned blue ambient shadow behind the writing
surface. Linux native fixture captures exercised both Codex and Claude focus
and toolbar-focus blur, dark/light themes, and 16/20px rem with reduced motion.
The desktop constrained the QA viewport to about 570px despite resize requests;
native wide-window appearance remains untested. The existing GPUI edge-alignment
fixture passed both providers at 1200/600/400px and 16/20px rem. Input submission,
compact-reply keyboard/draft/acknowledgment, and 33 UI conformance tests passed.

Live Codex and Claude provider boundaries, native compact-reply appearance,
Dashboard placement, hover/disabled/loading combinations, nondefault themes,
and macOS/Windows rendering remain untested. This change does not modify
provider transport or install/restart the user app.

CI repair follow-up: all 33 formerly failing application cases passed in
separate Linux fixture processes, and all 43 Shell fixtures passed on the
base-integrated binary. These cover moved Grid/Settings focus and blocking,
responsive close-confirmation retention, Dashboard draft focus and narrow live
activity alignment. Native composer captures also exercised compacting status
for Codex and Claude after integrating the newer input renderer. Native manual
sidebar/confirmation interaction remains untested. The glow was visually checked
against the exploratory option-B direction; no measured-reference certification
is claimed.

## Responsive sidebar fitting and transcript insets

Navigation and Checkout now yield space using rem-aware width budgets; explicit
reopening remains available, and automatic fitting preserves saved preferences.
Transcript/composer insets use the allocated transcript pane. This is an initial
responsive pass, not verification of every transcript item at every width.

The Linux compile check and local build/install passed. The initial GPUI test
build exhausted disk space; clearing Grove build caches allowed the navigation
fitting regression to pass. Final checks passed: navigation fitting/manual reopening,
Checkout automatic fitting/manual controls, transcript/composer edge alignment
for both Codex and Claude fixtures at 1200/600/400px and 16/20px rem, and all
33 UI conformance tests. The installed Linux binary matched the local build
by SHA-256. Native interactive inspection (hover/focus, repeated
resize, themes and zoom) remains untested because native computer controls were
unavailable in this session. Live Codex and Claude provider checks are untested;
layout fixtures do not establish provider-boundary behavior. macOS and Windows
remain untested.

## Session context panel

Model-selection label correction: Codex identity now follows the composer
override/configured model rather than the model saved with occupancy. The
focused regression covers Terra-to-Sol selection, configured-model reset,
saved-measurement fallback, unchanged occupancy/metadata, and preservation of
Claude's measured model identity. Four panel tests and all 33 UI conformance
checks passed on Linux. The isolated native capture test passed; Codex Dark
and Claude Light with increased zoom were visually inspected using saved-session
fixtures. Those captures establish card rendering, not a live model switch.
Live Codex and Claude provider switching and native picker interaction remain
untested for this correction; macOS and Windows were not exercised. No local
installation was performed, following the explicitly invoked shipping skill.

The simplified variant-C card shows occupancy for Codex and up to five used
category bars for Claude: Messages, System prompt, Tools & MCP instructions,
Memory files, and Skills & agents. Category percentages, nested details,
request accounting and tool-activity diagnostics are omitted. Unknown/missing
categories are not fabricated; zero remains distinct from missing data.

Passed: three focused panel tests (including category aggregation, child-count
exclusion, provider/session ownership, streamed occupancy, idle-only automatic
measurement, equal-count new generations and pending-query coalescing), 27 core
context tests (including stale replies, optional-query timeout/error, model and
compaction behavior), and 33 UI conformance checks. The shared GPUI fixture uses
normalized snapshot events to exercise Codex/Claude selection and query routing;
it does not establish actual Claude wire behavior.

Linux isolated native captures inspected both Codex and Claude at constrained
380px card width in Dark and 300px card width in Light with increased rem zoom.
These constrain the real card inside the compositor window; they do not establish
full narrow-window navigation behavior. The five bars and fixed numeric column
remained contained. The exploratory HTML is not a certified measured native
reference; certification is unsupported on this Linux capture backend.

Live production Rust adapters with disposable cwd/config: Codex 0.167.0 passed
two turns, with occupancy changing 26154 → 32769 and measurement revision 3 → 4.
Claude Code 2.1.220 returned an initial full measurement (6727 used tokens), then
the first model turn failed because its OAuth session expired and could not be
refreshed. Claude’s post-turn full-measurement freshness remains **unverified**
until sign-in is restored. Other Claude category combinations and MCP attribution
have fixture coverage rather than live coverage. Native-button-to-provider
end-to-end behavior, pointer scrolling and macOS/Windows remain untested.

Formatting, artifact policy and whitespace checks passed. The optimized local
build/install passed and the installed binary matched the build byte for byte.

## Sidebar activity group headings

The first icon-removal attempt targeted selected-session headers rather than the
sidebar activity group headings shown in the screenshot. That change was reverted;
activity group headings now omit their status icon. Native visual inspection and
live Codex/Claude provider checks remain untested (native computer-control APIs
are unavailable). The earlier two Linux Codex/Claude shared-header fixtures used
captured provider channels, not live providers.
The correction passed formatting, repository artifact policy and all 33 UI
conformance checks. Sidebar navigation fixtures passed 5 of 7 tests; two failed
because the `global-nav-new-session` target was absent. Their cause has not been
established, so the sidebar fixture suite is not fully verified.

## Home account contributions

Compact-header/local-time follow-up: the card now has an icon-only refresh at
heading right and the username immediately below the title. The calendar window
uses local today and sends each day's local midnight offset to GitHub (resolved
per date for DST), retaining GitHub contribution rules. An initial UTC-normalized
version failed the live sum check; preserving explicit local offsets passed the
124-day authenticated fetch. Five core regressions passed, including positive
and negative offsets; two UI tests (including long-account header geometry and
stable refresh bounds) and all 33 conformance checks passed. A six-run alternating
request-pattern comparison on the same 124 UTC dates/account measured serial
9.711/10.443/9.946s versus concurrent single-summary 2.838/3.230/3.564s; medians
9.946s and 3.230s. These are network-dependent loader measurements, not UI startup
benchmarks. Native visual inspection and manual refresh/tooltip/keyboard checks
remain unobserved because native computer-control APIs are unavailable in this
session; macOS/Windows and DST-transition runtime checks remain untested. Codex
and Claude live delivery were not exercised; this account card is provider independent.

Four-month follow-up: the inclusive UTC window now starts four calendar months
before today; 9px cells keep the grid within the compact card. A direct query
hit GitHub resource limits, so daily fields are now loaded in 40-day batches.
Each batch repeats the full-window totals and account identity; inconsistent
or errored batches are rejected. Four core tests, an authenticated 124-day live
fetch, three focused UI/theme tests and all 33 conformance checks passed. Native
Linux Dark/Light rest captures at 0.6 zoom showed the complete four-month grid
and summaries fitting the card. Formatting, artifact policy and the optimized
build passed. Native default-zoom/narrow-width interactions, manual refresh and
macOS/Windows remain untested for this follow-up.

Refresh-motion follow-up: the header uses shared Working and drawn-check marks
with account/action-scoped receipt tokens, the shared confirmation hold and
expiry, and a fixed action width. The contribution GPUI fixture now checks
unchanged action bounds across loading, success and restored rest plus the
reduced-motion static Working marker. Three focused tests, four shared receipt
tests and all 33 conformance checks passed. Native Linux timed captures observed
rotating work marks, success and restored rest in Dark, and static Working and
success marks with Reduce motion in Light, using real account loading at 0.6
zoom. Pointer-triggered manual refresh, the precise check-draw progression and
macOS/Windows remain untested.

Shorter-card follow-up: metrics are now two inline rows, Refresh is in the
heading, and the coverage note moved to the account-scope tooltip. Native Linux
rest captures in Grove Dark/Light at 0.6 zoom were inspected; the card was about
40% shorter in the matching captures. Three focused tests (including all theme
ramps), all 33 conformance checks, formatting, artifact policy and the optimized
build passed. Scope-tooltip hover and moved Refresh keyboard/pointer interactions
remain untested, as do macOS/Windows.

The Home-only account card uses live GitHub.com contribution totals through the
signed-in GitHub CLI account. Three core tests cover calendar-month boundaries,
summary/streak calculations, commit-only daily aggregation and rejected partial
responses; a separate authenticated live fetch passed. The final focused UI
suite passed three tests, including a density-level check, all 21 catalog theme
ramps (distinct levels and at least 3:1 occupied/empty contrast), and a Home-only
sign-in-state layout fixture at 1280px and 760px. All 33 conformance checks and
the optimized local build passed. An initial UI test build ran out of disk;
disposable compiler caches were cleared before the successful rerun.

Linux native rest captures use the final compact 10px cells, 2px gaps and live
account data at 0.6 UI zoom. All 21 catalog themes were
inspected together in a native panel contact sheet, with individual full-window
captures retained in the task's ignored artifacts. Grove Dark, Grove Light and
Tokyo Night Day were also inspected individually with the whole card visible. The installed local binary
matches the final tested build. Native default-zoom/narrow-width interaction,
day-tooltip hover, Refresh keyboard/pointer behavior and macOS/Windows remain
untested. The compositor rejected attempted QA window resizing; those attempts
are not native width coverage. Codex and Claude live sessions were not exercised;
this feature uses GitHub account data independently of either provider.

## Two-layer composer QA

The two-layer native composer retains the existing model/effort/speed card and
Access picker. Add context routes files/images into the attachment pipeline and
file, skill, and command prefixes into existing completion, preserving drafts.
The trailing model popup now anchors to the trigger's right edge so effort label
width changes do not move the slider during asynchronous updates.

Linux X11 QA used the optimized local binary with a disposable GROVE_CONFIG_DIR.
The Dashboard writing surface, joined destination strip, Add menu, reference
insertion, and existing model card were inspected in Grove Dark, including the
wrapping layout with Checkout open and a wider layout with it closed. The final focused composer rerun passed 40 tests with one ignored native fixture.
The model popup regression, context-meter strip check, model-label zoom fixture, Access
fixtures, and 33 conformance checks passed. Codex and Claude are verified by local
GPUI fixtures; live prompt delivery remains untested. Native light appearance,
full native zoom/keyboard-state coverage, and macOS/Windows remain untested.

Composer refinements passed five Access fixtures (including Codex and Claude
pointer selection), the destination sizing and project-picker draft-preservation
fixtures, the popup keyboard/anchor fixture, and 33 conformance checks. Linux
native inspection confirmed title-centered access icons, compact 16px text line
boxes with no extra gap, muted descriptions, square writing-box corners,
foreground-only toolbar/destination hover and open feedback, and project dropdown
initial badges with Home/Add icons for No project/New project. The Claude restore
fixture now clicks the rendered row bounds after scrolling rather than relying
on an obsolete fixed offset. These checks do not establish live provider delivery.

The task-jump regression `task_strip_docks_above_composer_toggles_and_jumps_to_task_start`
fails at the same transcript-alignment assertion on task-start f00b80f6 and the
updated build. It is excluded from the final focused composer rerun rather than
reported as passing. An initial parallel run overflowed a test thread stack; the
serial rerun uses RUST_MIN_STACK=16777216. A disk-full rebuild was retried after
removing task-created temporary build output. The ordinary installed app was not
replaced; delivery is the running isolated QA app.

Commands run from the repository root unless noted. Historical records below
are context for investigation, never fresh test results or product requirements.

## Mixed-content transcript rendering

Plan snapshots and Markdown sections are cached by accepted document/review
content; unrelated session updates no longer notify every retained plan. Generated
review-message summaries are cached within the owning snapshot. Static tables
reuse measured body/block geometry, including tables beside live code controls;
fixed-size user attachment thumbnails no longer force their entire row eager.
Native HTML, diagrams and interactive controls retain their existing safeguards.

Linux debug GPUI CPU replay (NoopTextSystem, seven warm samples per operation,
fixed saved inputs, 1000×900 test viewport) compared the task-start binary with
the final changes. These are CPU render/layout/paint timings, not displayed FPS,
GPU/compositor measurements, or live provider-boundary measurements:

| Workload | Baseline scroll p95 | Updated scroll p95 |
| --- | ---: | ---: |
| Saved conversation, 736 items | 304.85 ms | 221.94 ms |
| Twelve active plans | 830.52 ms | 706.44 ms |
| Mixed Markdown without plan controls | 423.27 ms | 305.34 ms |
| Plain prose, 200 items | 67.19 ms | 68.18 ms |

The saved-conversation reduction is 27.2%, below the proposed 30% p95 target.
The small plain-prose difference is inconclusive with this sample size. First
mount for the saved conversation fell from 2786 to 2451 ms (one sample each).
Further native frame-time attribution is still needed before declaring scrolling
smooth at the display refresh budget; full interactive-row virtualization has
not been introduced merely to reach the relative target.

Passed: 24 core plan tests; focused plan/approval/composer/preview checks; plan
snapshot reuse and selective-notification checks; mixed table/attachment cached
versus eager geometry and selection at narrow/default widths and changed zoom;
tables beside code controls with offscreen keyboard copy; streaming/manual
scrolling, static replacement, attachment sizing, transcript cache and HTML-return
checks; all 33 UI conformance checks. Codex and Claude are verified through local
native/transport fixtures only, not live provider sessions.

Linux native plan captures were inspected in dark/light themes with a table and
custom-answer composer. The requested narrow/zoom captures retained the same
pixel dimensions, and the zoom capture matched the normal image, so native
narrow/zoom appearance remains unverified despite the passing simulated checks.
The broader offscreen Tab-order test fails at the same assertion on the task-start
binary and the updated build; the new dedicated keyboard-copy check passed.
Live Codex and Claude scrolling/frame delivery, presented FPS, macOS and Windows
remain untested. Direct dependency-crate tests could not run from the excluded
workspace package; the changed table paths were exercised by Grove's integration
fixtures instead. The optimized local build, Linux bundle and installation
completed successfully; the user's running app was not restarted.

## Readable plan request summaries

Journaled plan-update and change-decisions requests render question/answer
summaries instead of protocol JSON, without changing stored or provider text.
Core checks cover exact-payload matching, custom-answer escaping, reloads, and
reopened decisions. Codex and Claude native fixtures verify readable transcript
text and unchanged source payloads. Linux light/dark captures were inspected;
live provider delivery, native narrow/zoom geometry, and macOS/Windows rendering
remain unverified for this presentation change.

## Plan discussion association

Section discussion actions now follow their grouped content and decisions and
name the target section. Eight plan fixtures (including Codex and Claude
composer/reference checks), 33 conformance checks, and the Linux native capture
test passed. Dark and light Linux captures were inspected for action placement.
Live Codex and Claude delivery, native narrow/zoom geometry, long-heading layout,
and macOS/Windows rendering remain unverified for this change.

## Long plan decision prompts

Decision prompts use their full content width above the Discuss action; the
compact action yields to the pane width while retaining its complete accessible
name and discussion target. Nine native plan tests passed (one screenshot test
ignored in the ordinary run), together with 33 UI conformance checks, formatting
and repository artifact checks. Codex and Claude native fixtures exercise long
QA prompt geometry and existing composer behavior; they do not establish live
provider delivery.

The macOS `views::native_plan::tests::long_decision_native_snapshots` fixture was
also run explicitly and passed. Inspected light/dark captures at 1000px and
400px with 16px rem showed readable prompts and bounded Discuss actions for the
provider fixtures. Captures are under `.grove-artifacts/long-plan-decision/`.
All 24px rem captures were blank theme backgrounds, so zoom visual coverage
failed and remains unverified despite the capture test passing. Live Codex and
Claude delivery and Linux/Windows native layout remain untested for this fix.

## Claude slash-command payloads

Claude submissions beginning with `/` preserve the command and its arguments
without appended Grove review-summary context, including host `/compact`.
Transport regression fixtures check the full outgoing payload; ordinary-message
queue fixtures retain review context. Live Claude command recognition and native
composer interaction remain untested for this fix. Codex transport is unchanged;
its unit coverage does not establish live provider behavior. No platform-specific
code changed; macOS and Windows execution remain untested.

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

Transcript file-link explorer visibility: the shared sidebar now opens the owning
explorer with a file viewer and returns to the side panel when the viewer closes.
Regression fixtures cover Codex and Claude browser-to-transcript event routing,
initially hidden/shown explorer states, and nested-file ancestor expansion without
broadening the root for outside-checkout links. Live Codex and Claude provider
checks and native pointer/keyboard/screenshot QA remain untested for this change;
fixture assertions do not establish those outcomes.

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

## Section-scoped Describe and Review menus

The checkout's Local changes and Pull request headers now own persistent overflow
menus. Ask command routing, prompt settings, reader, core generation, smoke examples
and bundled guidance were removed. A repository identifier sweep found no remaining
Ask feature references; general provider questions and Side chat are separate
features and remain supported.

Linux fixture checks cover source-specific menu entries, Enter/Escape and focus
restoration, native Tab-stop traversal, saved/stale/working states, retained results
on clean/detached sources, and a retained explicit PR alongside a different attached
PR. Codex and Claude fixture cases preserve draft/session ownership and verify
composer/Dashboard routing without submitting the originating draft. The focused
sidebar, retained-controller, routing, Settings and all 33 UI conformance checks
passed. Changed Rust formatting and repository artifact checks passed.

Native X11 fixtures exercise actual Grove headers, menus and retained readers.
Full-shell captures show the new placement and keyboard reader round trips for
Codex and Claude fixtures. Isolated header captures cover dark/light and an actual 180-pixel-wide component.
The requested native zoom capture did not establish a changed rendered scale.
Window-manager tiling prevented the requested full-shell resize;
full-shell captures labelled light still rendered dark, so those labels do not
establish full-shell narrow/light verification. These captures are exploratory
native QA, not a certified macOS measured-reference parity result. macOS and
Windows were not exercised.

Live Codex Describe and Review retained-conversation smokes passed canonical
reload, pending cleanup, current snapshots and unchanged fixture HEAD/index/work
state. They bypass the new menus and have no originating composer: live menu
invocation and draft preservation are not established by them. Claude reached the
normal provider boundary but both attempts failed with expired OAuth credentials;
Claude live generation remains unverified. No account configuration was changed.

A broader core run passed 872 tests with 9 ignored and 3 failures. The agent-path
and recovered-GitHub-CLI tests also fail individually because installed executables
are selected instead of their fixture executables; their owning source files are
unchanged by this work. The tmux environment test passed individually after failing
in the parallel run. The broad suite is therefore not a clean pass. Logs and native
evidence for this run live under ignored `.grove-artifacts/change-tools-exploration/`
and the section-menus/section-menu native capture directories.

The review-specific isolated menu capture retained an earlier frame; it is not
independent visual evidence for Review. The inspected shared menu capture shows
both Describe and Review entries, while Review reader/session behavior is covered
by fixtures. Final `./install.sh` succeeded on Linux using the local profile;
`~/.local/bin/grove` matched `target/local/grove` by SHA-256 after installation.

## Main-conversation MCP connection warnings

Codex and Claude are **verified live on Linux** with the production Rust
transports and installed Codex 0.167.0 / Claude Code 2.1.220 CLIs. The
`mcp_connections_smoke` example configures a deliberately failing stdio server:
both transports emitted a visible transcript warning naming the server with its
connection error while retaining Idle conversation state and no fatal error.
The smoke makes no model requests and uses no Jira credentials. Build with
`cargo build -p grove-core --example mcp_connections_smoke`, then run each provider
with a separate disposable `GROVE_CONFIG_DIR` and cwd.

Regression fixtures cover Codex thread isolation, duplicate suppression and
reauthentication guidance; Claude initialization inventory, health polling,
unsupported/unanswered queries, recovery and configuration acknowledgment while a
health request is pending. Both primary regressions failed before implementation.
The core suite passed 878 tests with 9 ignored, excluding two environment lookup
tests that also failed on unchanged `353397b`. Strict Clippy emitted the same 31
diagnostics on that baseline and this change. All 33 UI source conformance checks,
formatting and the artifact policy check passed. The existing GPUI provider-warning
layout fixture passed at normal/narrow widths for both Codex and Claude; this is
fixture evidence, not desktop visual inspection.

Remaining gaps: authenticated Jira on the user's other machine, model-turn Claude
verification (local provider OAuth is expired), macOS/Windows runtime behavior,
older CLI versions, and desktop visual inspection are untested. The fix exposes
provider-reported failures; it does not establish or repair the remote Jira
failure's underlying authentication/network cause. The default local installation
completed successfully after the final code changes.

## Prompts Settings removal (2026-11-06)

The Prompts category, editor, draft state and dedicated fixtures were removed.
Built-in generation and saved override compatibility remain. Linux verification:
18 Settings-related tests and 33 UI conformance checks passed, along with
formatting and repository artifact checks. The existing isolated native Settings
capture fixture showed General, Agents, Terminal, Privacy and About in light and
dark at rem 16. Its rem 24 capture was blank, so enlarged-zoom native visual
inspection remains unverified; GPUI geometry tests passed. Evidence is in
`.grove-artifacts/remove-prompts/` and the fixture's external temporary captures.
macOS and Windows native rendering were untested. Codex and Claude live provider
behavior was untested; this removal changes only Settings UI, and no live
generation was exercised.

Checkout menu alignment follow-up (2026-11-06): the Linux GPUI Checkout layout fixture passed, including equal trailing bounds for Local changes, Git and Scripts overflow controls, and absence of the duplicate Add script row. The script register/editor interaction fixture and all 33 source conformance checks passed; repository artifact policy passed. The layout fixture's stale expectation for a standalone Describe changes row was updated to the existing section-menu behavior. Actual native screenshot/hover/focus inspection, light/dark and zoom appearance, and macOS/Windows rendering remain untested because native computer control is unavailable. Codex and Claude were exercised only as session fixtures within the layout test; live provider behavior is untested for both and no provider transport behavior changed.

## Grouped transcript provider failures

Adjacent provider diagnostics now share a collapsed disclosure, with literal
saved notices preserved. Linux GPUI fixtures verified Codex and Claude grouping,
separation at intervening messages, click/Space/Enter expansion and shared text
coalescing; existing rail checks passed at 1000/560 widths for both providers.
The focused provider suite passed 23 tests and all 33 UI conformance checks passed.
Live production-transport MCP startup failure probes passed for **Codex** and
**Claude**, each retaining an Idle conversation with a provider notice; these
probes do not exercise native UI input or authentication recovery.
An isolated Linux X11 fixture was inspected collapsed/expanded in Grove Dark and
expanded in Grove Light. The capture requested rem 20 for the light frame but
rendered geometry stayed unchanged, so changed-zoom native appearance remains
unverified. Native narrow-window, hover/focus contrast, screen-reader interaction,
macOS and Windows remain untested. Artifacts and logs are under
`.grove-artifacts/error-disclosure/`. Expand/collapse is immediate input feedback,
matching existing transcript work/tool disclosures.

HTML browser action: Linux `cargo check` and repository artifact policy passed.
The persistent inline/expanded action and fenced-document export still need native
visual and keyboard checks (normal/narrow widths, themes and zoom), browser-launch
interaction checks, and live Codex and Claude transcript checks. Neither provider
was exercised live for this change; macOS and Windows remain untested.

## Contextual plan decisions and finalization (2026-11-06)

The plan reader now places decisions beside Markdown sections, routes custom
answers and discussion through the existing composer, and removes the duplicate
plan-ready/review form. Exact submitted answers must be incorporated into a new
immutable finalized revision before approval. Changing finalized decisions
revokes readiness and requests a new decision draft. Unsaved custom answers and
the ordinary text/image/reference draft are retained independently.

Linux verification: 37 core plan tests passed. Native GPUI fixtures exercised
Codex and Claude custom-answer events, save/cancel and delayed acknowledgment,
stale unrelated errors, draft restoration, exact-revision discussion, read-only
peek, keyboard/source focus, narrow/wide footer geometry and approval gating.
All 33 UI conformance checks and repository artifact policy passed.

Live **Codex** production transport generated a two-decision plan, accepted mixed
selected/custom answers through one update despite duplicate submission, imported
a finalized revision and restored readiness after persisted-session reload.
Approval readiness was checked on a clone; no live implementation approval was
sent. Live **Claude** was attempted but blocked before generation by expired
OAuth; its provider boundary remains unverified. Native fixtures do not establish
live widget-to-provider end-to-end behavior for either provider.

Isolated Linux native captures and logs are under
`.grove-artifacts/plan-implementation/`. macOS/Windows, screen-reader behavior,
measured focus contrast, and cognitive-load benefits remain unverified. No
reference-parity certificate is claimed for the exploratory mock.

The ignored Linux capture fixture passed after releasing the focused platform
input handler before window shutdown; earlier runs captured the UI but failed
cleanup. Dark/light plan and custom-answer screenshots were inspected. Requested
narrow/rem-20 captures retained the compositor's original geometry, so native
narrow/zoom visual coverage remains unverified despite fixture geometry checks.
The final focused application run passed 8 tests (2 desktop capture tests ignored);
the dedicated Linux capture passed separately. Five native-plan unit/fixture
checks also passed. Immediate answer/focus/disabled transitions retain existing
input-feedback behavior; earlier revision disclosure is immediate.
The final `./install.sh` local-profile build and bundle succeeded, installing the
Linux executable and desktop launcher under the user's local application paths.

## Long transcript performance (2026-11-09, in progress)

The current optimization work retains prepared plan content, measured offscreen
plan/tool geometry and real keyboard actions, caches rich-message/work indexes,
and retains mounted diagram previews independently of the shared raster cache.
An ordered-insertion fast path in the existing vendored sum_tree reduces focus
registration costs; its 11 tests passed, including retained-snapshot and mixed
insertion/replacement/removal parity with BTreeMap.

Linux native CPU scene-build measurements expose the **entire** history and use
normal view invalidation rather than a full-window refresh. At the compositor's
actual 569.6 × 679.2 logical viewport, 21-sample medians for 1/12/48 rich plans
were 6.92/14.16/45.55 ms, versus 9.27/76.18/311.78 ms with offscreen-plan reuse
disabled in the same optimized test binary. These are scene-construction times,
not GPU/compositor FPS; long histories still materially exceed short histories.
Earlier 48-plan measurements that exposed only the latest 20 turns are not full
history evidence. Offscreen-diagram reuse subsequently reduced the same-binary 48-plan median
from 44.41 to 30.57 ms (p95 53.19 to 36.91 ms).

Four offscreen-plan fixtures passed for **Codex and Claude**, covering real
control order/actions and dynamic diagrams. The new diagram geometry/keyboard
fixture and all 18 Mermaid tests passed. Earlier transcript selection, footer,
minimap, tool, index and conformance checks passed; the current targeted pass covered 78 application tests, including both-provider
message/tool behavior, plans, diagrams, selection, streaming, minimap and all
33 conformance checks. This was not a full application test run. Native wheel-event checks on 1/48-plan histories
preserved all items and returned exactly to the initial scroll position. A
before/after native screenshot for the sum_tree change had zero differing pixels;
this is one viewport, not comprehensive visual or interaction certification.

Live **Codex** plan-finalization transport passed generation, selected/custom
answers, duplicate suppression, persisted reload and stale approval gating;
readiness was checked without sending implementation approval. Live **Claude**
was attempted but blocked by expired OAuth. Neither result establishes live
widget-to-provider coverage for all transcript components. Linux native mixed-content benchmarks completed, but compositor smoothness,
remaining native interaction states, changed-width/theme/zoom appearance, and
macOS/Windows remain unverified. The final local-profile production build and
`./install.sh` succeeded; the installed executable matched the built executable
by SHA-256. The running user app was not restarted.
Temporary logs and workloads are in `.grove-artifacts/performance-followup/`.


Offscreen message/footer/work-header keyboard aliases now avoid pointer and
hover registration. The 2,000-message median fell from 152.19 to 106.13 ms in
successive optimized matrices; 200 messages fell from 15.90 to 9.31 ms. These
were separate runs with ambient system load, not compositor FPS measurements.
The saved 736-item session measured 14.71 ms collapsed and 40.23 ms with all work
groups expanded. A 1,000-tool history measured 16.46 ms; 2,000-paragraph and
2,000-line-code bodies measured 14.29 and 20.41 ms. Short/long parity is **not
achieved**. Full-history layout/dispatch traversal remains a material cost even
when only a few message bodies are materialized per frame.

The latest native wheel runs processed 120 events/frames in each 1/48-plan
history, moved to -1440 logical pixels and returned exactly to zero without item
loss. Median CPU draw time was 8.08/28.98 ms (p95 9.68/34.02). Matched native
captures for the message-alias change and for a ready diagram scrolled offscreen
and back had zero differing pixels. Additional plan viewport captures also
matched. This covers Linux dark-theme captures at the actual compositor viewport,
not all themes, widths, focus/hover states or physical display presentation.

A fixed-height layout experiment preserved scroll extents and passed the
both-provider message regression, but its small 3–5% CPU change was inconclusive
against run-to-run variation; it was reverted. Earlier kernel sampling lost too
many events to support percentages; retained findings use native timing,
opt-in phase measurements and debugger stacks instead. Upstream GPUI view-cache
and variable-height-list implementations informed the investigation; no new GPUI
version or blind upstream patch was adopted. The next unresolved work is bounding
full-history framework traversal and verifying actual presentation smoothness.

Composer follow-up: equal checkout-strip vertical padding passed the native destination bounds test, including vertical center alignment. Removed tooltips from composer toolbar, context meter, attachment/reference actions, model picker, and destination selectors while retaining accessible labels. The three model-picker tests and 33 conformance checks passed. Rebuilt the isolated Linux QA app and inspected wide/narrow Dark layouts and project hover with no tooltip. Live Codex/Claude delivery and other platforms were not exercised by this visual-only change.

Project/worktree menu polish: reduced the project badge from 28px to 20px and its initials to the micro type role, with matching Home/Add leading slots. Added catalog branch icons to worktree options and Add to New worktree, retaining trailing selection checks. Final local build and artifact checks passed; both menus were inspected in the refreshed isolated Linux Dark QA app. No provider or other-platform checks were run for this visual-only follow-up.

Composer order follow-up: moved model/effort/speed beside Add and Access beside Send/Stop, with popup anchors following the stable outer edges of their new groups. The final local build, asynchronous effort-reset regression, popup keyboard/anchor regression, and Codex/Claude access pointer-dispatch fixture passed. Inspected wide/narrow native Linux Dark layouts and the model card in the refreshed isolated QA app. Live provider delivery and other platforms were not tested.

Latest composer grouping: model/effort/speed and Access are left aligned; Add and Send/Stop are grouped on the right, with Add anchored to its trailing edge. Project monogram lettering is 9px inside the existing 20px badge. The final local build, popup keyboard/anchor regression, Codex/Claude Add draft-preservation fixture, Codex/Claude access-dispatch fixture, and 33 conformance checks passed. Inspected native Linux Dark wide/narrow layouts and the smaller project monogram in the refreshed isolated QA app. Earlier detached QA launch attempts exited; the final app was launched in a retained process session and its window was inspected. Live provider behavior and other platforms remain untested.

Full-height composer menus: removed internal scrolling and height caps from Add/Access, project/worktree destinations and the model catalog, including obsolete model scroll offsets. Four model-picker tests passed, including an eight-model catalog exceeding the old cap with the last row visible and stationary under wheel input. Claude Restore defaults was clicked without scrolling, popup routing passed, and 33 conformance checks passed. Rebuilt and launched isolated Linux QA and inspected the full worktree menu through New worktree. Very large menus exceeding a short viewport and other platforms remain unverified; full-height menus intentionally have no internal overflow fallback. Live Codex/Claude delivery was not exercised.

Composer geometry follow-up: replaced Add plus with the catalog paperclip, restored all four writing-surface corners to the shared panel radius, and centered Dashboard/session context strips at 90% width. Built and inspected native isolated Linux Dark wide/narrow layouts and opened the paperclip context menu. The real-composer 90% width/centering regression, destination geometry fixture, Codex/Claude Add draft-preservation fixture, and 33 conformance checks passed. The first center assertion used a root-only strip fixture; the regression now measures the actual composer parent. Live Codex/Claude delivery, other appearances and platforms were not exercised.

Popup motion and Dashboard polish: access icons now use ICON_MD with SPACE_LG gaps; the Dashboard composer starts at 28% and the heading is “Let's get it done.” Built a self-contained three-treatment motion exploration from native-export colors/font data and catalog SVGs, tested browser replay/dismissal/reopen/interruption/reduced-motion and narrow overflow, and opened it in the existing browser. The selected B short rise is implemented for composer context/access, model catalog/effort/speed and project/worktree popup surfaces: 140ms entrance, 80ms retained dismissal, source-scope/generation guards, and immediate reduced-motion endpoints. The context strip is centered at 95% width. Native PopupMenu exit instances omit input/focus listeners; effort exit rendering no longer recreates drag state. Full-height speed-card geometry and Side chat alignment fixtures now measure the approved no-scroll/two-layer layout.

Final checks: 52 composer tests passed, one native-capture test was ignored, and the unrelated native-launch fixture and previously documented task-jump baseline failure were excluded from this focused run. Two popup lifecycle tests, four model-picker tests and 33 conformance checks passed; build, formatting and artifact checks passed. Refreshed the isolated Linux QA process with the final functional build. Codex and Claude fixture behavior is verified; live provider delivery and non-Linux coverage are untested. Native visual animation inspection remains unfinished: native computer-control APIs are unavailable in this session. Browser observation of the local mock in the user's browser was separately blocked by the file URL policy; the browser mock checks above ran before that restriction was observed, and no bypass was attempted.

Dashboard copy/placement follow-up: changed the heading to “Ship fast. Break faster.” and moved the headline/composer/following content together by raising the viewport fraction from 28% to 31%. The local application build, formatting, diff and artifact checks passed; relaunched and verified the isolated QA process. The targeted placement test could not run because test-binary linking failed with the filesystem full; removed the failed temporary linker output. Native visual inspection remains unavailable. Live Codex and Claude behavior and other platforms were not exercised by this static copy/placement change.

Sidebar layout revision: native rendering, narrow widths, themes, zoom, window controls and workspace/settings/grid interactions remain untested. Codex and Claude live session navigation are untested; this change was checked by compilation only.

Composer spacing and queue count placement: compilation and artifact checks cover the source change; native rendering at narrow widths, themes and zoom remains untested. Live Codex and Claude composer/queue interactions remain untested; provider behavior was not changed.

Thin catalog icons: native raster inspection at small sizes, light/dark themes and changed rem zoom remains untested. Codex and Claude live transcript/composer rendering is untested; the further reduction to a 1.0 stroke and outline Grid/Settings/Play/Initialize passed all seven icon catalog tests and the repository artifact check. The source audit covered app SVG/icon call sites and the shared asset loader; this does not establish screen-by-screen native visual compliance.

Standalone session-view icons and side-panel clearance: the Checkout native GPUI layout fixture passed with the card below the toolbar controls; compilation and artifact checks passed. Native visual inspection of hover/focus, wide/narrow layouts, themes and zoom remains untested, as do Side chat visual states and live Codex/Claude provider interactions.

Sidebar worktree-group headings use the branch icon; activity-group status icons reuse the shared status renderer for Codex and Claude sessions. Native visual inspection, reduced-motion rendering, narrow widths, themes and zoom remain untested; live Codex and Claude status transitions were not exercised.

## Main New session entry

New session actions now open the Dashboard directly while preserving its draft.
The alternate empty Codex/Claude conversation introduction and starter cards were removed.
Live Codex and Claude provider checks and native interactive visual inspection are
untested for this change; source and GPUI fixtures do not establish those boundaries.
macOS and Windows are untested.

Linux compilation, formatting, artifact policy and 33 UI conformance checks passed.
The Codex/Claude empty-view fixture and both main-screen navigation fixtures passed.
The broader new-session run exposed an obsolete switcher expectation (updated) and
a sidebar fixture missing `global-nav-new-session`, consistent with the existing
sidebar verification gap above. The Linux local installation succeeded.
The corrected switcher navigation fixture passed on rerun. Installed and built
Linux binaries matched by SHA-256.

Main New Session layout restoration: restored the earlier Dashboard composition
(composer at 31% of its viewport, destination tray attached, Activity below) and
downward Activity entrance, while retaining the updated empty transcript session.
Linux: 15 of 16 Dashboard fixtures passed, including Codex and Claude draft,
worktree round-trip, sequential adoption, reduced motion, and placement checks.
The unchanged standalone destination-control fixture failed its pointer-exit
assertion (`[false, true]` rather than `[false, false]`); hover exit is unverified.
The empty-session fixture passed for both Codex and Claude, and all 33 UI
conformance checks passed. Native screenshot capture failed because this build
has no HeadlessRenderer configured; visual inspection, themes/zoom and native
interactive checks remain unverified. Live Codex and Claude provider boundaries
and macOS/Windows remain untested. Formatting and artifact policy passed.
The default Linux local build/install succeeded; installed and built binaries
matched by SHA-256.

## Double-Escape session interruption

The active Codex/Claude conversation now arms a three-second confirmation on
unmodified Esc, then sends the existing Interrupt action on a second distinct
press. Linux GPUI fixtures passed for both providers: one interrupt only, draft
preservation, held-key rejection, typing/timeout/focus/turn cancellation, menu
precedence and IME dismissal. The narrow/light/increased-rem fixture measured
the hint within the composer column, with Reduce motion. These are simulated
GPUI input/layout checks, not desktop visual or live widget-to-provider evidence.
Fourteen focused regressions and 35 additional checks passed, including all
33 UI conformance checks, existing Stop behavior and composer popup routing.
Formatting, whitespace and repository artifact checks passed.

Live **Codex** production-adapter smoke passed a streamed turn, persisted resume,
real interruption and recovery cleanup in disposable configuration/cwd. Live
**Claude** initialized a session but its first model request failed because its
OAuth session expired and could not be refreshed; interruption is **unverified**.
Neither smoke drives Esc through the installed UI. Native visual inspection is
unfinished because native control APIs are disabled and no native app surface
is exposed. macOS/Windows and native theme/zoom/interaction appearance remain
untested; Linux layout fixtures do not certify visual parity with the exploratory
HTML mock. Existing unrelated checkout edits were preserved.

The final `./install.sh` local build/install succeeded on Linux. The installed
`~/.local/bin/grove` and `target/local/grove` matched by SHA-256. The running
user application was not restarted. No release was created.

## Installed skill discovery refresh

The skill catalog now watches global and ancestor project skill sources in the
background, including symlinks and plugin installations, and updates open Codex
and Claude composers without reconnecting their conversations. After rebasing onto `feat/new-sidebar`, 11 catalog tests, two actor tests and
live Codex/Claude global/project install-removal metadata checks passed. The
GPUI queued-snapshot regression passed for both providers, preserving draft and
thread identity. Linux build/install passed. Native UI interaction verification
remains pending;
core Clippy reports 33 findings in existing feature-branch code. Live catalog probes exercise the provider
metadata adapters, not the installed composer or an active model turn.

## Checkout Git failure ownership

The action-owned Git failure change was checked on Linux against parent
`07143a0b8b9710d09295d6dd43c62b5b434b32bc`. Core Git tests passed 11/11,
including an actual rejected push and divergent fast-forward-only pull against a
local remote. Checkout tests passed 18/20; the Agents disclosure test and the
later narrow-layout portion of the Checkout/PR test fail at the same assertions
with the parent Checkout source restored (16/18 baseline tests pass). The added
PR-confirmation ownership assertion executes before that existing narrow-layout
failure. Conformance passed 33/33. These results do not establish a green full suite.

The actual Sidebar Linux capture ran with frozen Codex and Claude sessions.
Inspected screenshots show one failure and distinct recovery guidance beneath
Pull for Codex in dark mode and Claude in light mode, including retained Pull
context with zero remote commits. Requested window widths and zoom did not
reliably reach the rendered frame: every capture remained 1424 by 1698 pixels.
Narrow-width and zoom coverage remain unverified. The native accessibility tree
was unavailable without an active accessibility client, so accessibility-based
uniqueness assertions were not exercised. Full hover/focus/press, failure/retry,
and confirmation interaction combinations remain unobserved in a native window.
Codex and Claude live provider boundaries were untested; these are provider
fixtures around provider-independent Git presentation. macOS and Windows remain
untested. Raw Git stderr is intentionally withheld; no diagnostic disclosure is
shown without distinct safe diagnostic content.


## Sidebar header ordering follow-up (2026-11-09)

The workspace selector now follows the window controls, left-aligns its name
and chevron, and yields trailing space to the sidebar expand/collapse control.
This delivery did not exercise native screenshot, hover/focus, theme, narrow
width or zoom appearance checks; native computer control is unavailable.
Codex and Claude live provider checks are untested for this layout-only change;
provider transport behavior is unchanged. macOS and Windows remain untested.

Delivery checks: formatting, diff whitespace, repository artifact policy and all
33 UI source conformance checks passed. The shell suite completed with 25 passed
and 18 failed (header/settings selectors, rail geometry, collapse, shortcuts,
Zen/grid and worktree input expectations); no baseline comparison established
whether these failures predate this layout change. This is not a green suite.
The final Linux local build/install succeeded; installed and built binaries
matched by SHA-256. The running application was not restarted.

## Response-based suggestion carousel

The revised shared Codex/Claude composer stores a variable set of short
response-part follow-ups, hides it for any nonempty draft, and restores its
selection when the draft is cleared. Candidate generation now returns a bounded
array; native controls and empty-composer left/right navigation select a reply.

Linux verification passed five core candidate/schema tests, seven generation
transport tests, eight native composer fixtures, three adjacent file/skill/slash
Tab-completion fixtures, and all 33 UI source-conformance checks. Codex and Claude
are **verified through shared native transport fixtures** for typing/clearing,
Tab/Undo restoration, chevron and arrow wraparound, successful-send retirement,
empty Enter, stale replies, generation while typing, modifier/menu/marked-text
priority, single-result disabled controls and shell-owned Shift+Tab traversal.
An initial bare-fixture Shift+Tab failure was traced to its missing production
shell traversal handler; the corrected host preserves strict empty-draft/focus
assertions and passed. This was a test-host correction, not a production fix.

Five fresh isolated Linux X11 captures were inspected in Grove Dark and Light,
including selected/restored suggestions, 300px conversation width and rem 20.
They show short ghost replies and category-free pagination, with wrapped hints
at narrower widths. The compositor retained its window size, so constrained
conversation content is verified rather than full-window resize behavior. The
production build, formatting, whitespace, repository artifact policy, 85 strict
reference-verifier tests and 25 artifact/install/CI tooling tests passed.

**Codex:** live isolated production structured-array generation passed with two
suggestions (36 and 38 characters). **Claude:** live generation was attempted but
failed; `claude auth status` reported signed out (`loggedIn: false`). Successful
live Claude generation remains **untested**. Provider generation and native
composer fixtures were exercised separately, not as an end-to-end live session.
Actual OS IME input, screen-reader use, desktop pager hover/focus appearance and
macOS/Windows runtime behavior remain untested. No local installation or user-app
restart was performed. Prior single-candidate evidence below describes the old
behavior, not verification of the carousel.

## Suggested next replies

Linux verification passed 12 focused core/generation-transport tests and 51
native reply, completion, focus-traversal and design-conformance fixtures. These
cover Codex and Claude fixtures, exact-prefix persistence, divergence/Escape,
whole-reply Tab acceptance, caret placement, Undo, stale acceptance and coalesced
submission/completion handling. The production binary build passed. The adjacent
worktree-input test's obsolete Projects selector failed on the untouched base;
its selector was corrected and the test then passed.

Fresh isolated Linux capture processes inspected empty, matching-prefix,
constrained-width and multiline states for Codex and Claude, in Grove Dark and
Light, including rem 20. The compositor retained its window geometry, so these
captures constrain the actual conversation pane rather than claiming full-window
resize coverage. Ghost text sampled from native frames measured 5.32:1 contrast
in Light and 7.78:1 in Dark. Earlier multi-state capture attempts reused stale
compositor frames; the supported fixture selects one state per process with
`GROVE_NEXT_REPLY_CAPTURE_CASE`. Focus is released before teardown.

**Codex:** isolated production generation with CLI 0.167.0 and `gpt-6-luna`
returned a valid structured suggestion. **Claude:** CLI 2.1.220 attempted the
`haiku` request but its OAuth session expired and could not refresh; successful
live Haiku generation remains unverified. Native provider fixtures and live
backend requests were exercised separately, not as an end-to-end live composer
session. Actual screen-reader and IME interaction, long-preview scrolling, and
macOS/Windows runtime behavior remain untested. Windows process-tree termination
follows the existing title-generation cleanup pattern but lacks runtime evidence.

Strict workspace Clippy failed with 32 diagnostics in unchanged core files. Every
one reproduced at the same file, line, column and message on the untouched base;
this change does not establish a clean whole-workspace lint result. Formatting,
repository artifact policy, reference-verifier tests and artifact-tool tests passed.
CI spelling and supply-chain jobs also fail on unchanged baseline inputs; a
locked dependency has been yanked. No local installation or user-app restart was
performed.

## Drag-to-Applications macOS disk image (2026-11-11)

The selected installer uses the supplied canopy artwork, composited at 720×440
with a green curved arrow. The picture lives inside the staged app's Resources,
so the volume root contains only Grove.app, the Applications symlink and Finder's
reserved .DS_Store. Packaging copies the input app, adds the picture, then ad-hoc
signs and strictly verifies that copy; the original input app remains untouched.
This preserves the existing release pipeline's ad-hoc signing semantics, not
Developer ID signing or notarization. Non-ad-hoc signed input apps are rejected:
a future Developer ID pipeline must inject the picture before its signing step.

Local canopy packaging, focused HFS+ auto-open metadata tests, shell syntax,
artifact policy and whitespace checks passed. Finder close/reopen checks assert
manual arrangement and both icon positions before image conversion. No app
installation or release was performed; GitHub-hosted runner packaging remains
untested. Independent packaging and native Finder inspection of the compressed
canopy image verified the selected art at the correct scale, the green curved
arrow, and only two visible items with aligned icons and labels: Grove on the
left and Applications on the right. No clipping, scrollbars or helper files were
visible. The mounted app and the original installed input both passed strict
signature verification; the input contains no added installer background. Root
contents were limited to .DS_Store, Grove.app and the /Applications symlink.
Ordinary Finder opening previously mounted the image but required manual volume
navigation; automatic window opening remains unverified despite its metadata hint.


## Review brief panel-only presentation (November 2026)

Dedicated review-brief blocks are filtered from assistant transcript prose,
response Copy and minimap previews while raw responses remain persisted for panel
extraction. Codex and Claude share this presentation path. Automated fixtures
cover both provider identities, retained raw text and brief data, streaming
prefixes, and the attached panel's wide/narrow/zoom layout and review action.
Live Codex and Claude provider-boundary checks remain untested. Native screenshot
inspection in light/dark themes and changed zoom remains untested on macOS,
Linux and Windows; automated layout fixtures do not establish pixel appearance.

A macOS `review_` filtered run built successfully and printed 31 passing tests,
including the new panel-only regression and attached-panel fixture. The existing
`review_hold_waits_400ms_and_cancels_on_return` timer test then remained stalled
for more than two minutes; the coordinator terminated the test process. No
baseline run established whether this stall predates the change. A rerun excluding
that timer test passed all 31 remaining review tests. All five core brief tests,
33 source-conformance checks, formatting and repository-artifact checks passed;
application and test compilation succeeded.


## Linux OS text preferences

The Linux bridge supports `org.gnome.desktop.interface/text-scaling-factor`.
A private D-Bus/dconf regression test exercises the production GSettings
subscription at startup, increase/decrease, reset, and absent-schema fallback
without modifying desktop preferences (`scripts/test-linux-text-preferences.py`).
Rust checks cover fractional text factors independent of app zoom, consistent
native rem/component-theme and integrated-report CSS sizes, and removal of the
GNOME text factor from the Xft display-DPI fallback. Explicit GPUI/GDK display
scales are unchanged. Manually configured Xft DPI alongside unrelated GNOME
settings is ambiguous; use an explicit GPUI/GDK display-scale override there.

Linux isolated X11 Dashboard captures at text factors 1.0 → 1.5 → 0.8182
showed text, containing controls and responsive layout grow and shrink in the
same 1402×1708 physical window with GDK_SCALE=2. The display was asleep; a
nondestructive Expose event to the QA window was needed to obtain fresh frames,
so this does not prove unattended compositor redraw while DPMS is off.
The QA used a private D-Bus/dconf/config and sent no provider messages.

Actual WebKit DOM sizing after a text preference change, terminal selection
and pointer alignment, mixed-DPI moves, KDE/XSettings-only preferences and
Windows/macOS OS text settings require additional verification/support.
Codex and Claude transport behavior is untested by these platform sizing checks;
the native/report size fixture does not establish live provider-boundary behavior.

Strict local Clippy was blocked by two existing warnings also reproduced on
base 1852a77: `semicolon_if_nothing_returned` in codex_view.rs and
`assigning_clones` in file_browser.rs. No unrelated lint changes were included.


## HTML transcript browser actions

Local HTML transcript links retain their viewer link and expose an independent
Open in browser action, including when rendered from saved Markdown without an
automatic preview. Explicit browser labels keep one direct action. Missing targets
and browser-launch errors are shown beside the owning message/link. Standalone
custom links use native paragraph layout so long labels wrap to the parent width.

Linux evidence: 29 HTML-related tests, the shared Codex/Claude local-file viewer
fixture, and 33 conformance tests passed. GPUI renderer/input fixtures cover
arbitrary/formatted labels, HTML/HTM and case variants, line/fragment references,
spaces, independent actions for multiple files, fresh-session rendering, mouse
viewer selection, browser Enter/Space retries, missing targets, and visible launch
failures using the test backend. The long-label bounds regression failed before
the standalone-link layout correction and passed after it. Existing fenced HTML
and integrated-report tests passed.

Inspected isolated native Linux X11 captures of the actual link component in
Grove Dark at 700 logical-pixel content width and Grove Light at 280. The window
manager ignored requested window dimensions, so the fixture constrains content
width directly. Captures confirmed wrapping, leading browser icons, left alignment
and a single explicit browser action. Sampled full-color link pixels against the
native background measured 8.33:1 in Dark and 7.08:1 in Light.

Live Codex and Claude provider delivery remain untested: the provider checks above
use synthetic snapshots, not live transport. Actual OS browser handoff, native
hover/focus, scrolling/restoration in a live session, and macOS/Windows native
interactions remain untested; desktop interaction control is unavailable here.
The original screenshot’s apparently plain label was not reproduced in the
running application. Its saved response contained a valid HTML link and its file
existed; the native component fixture renders that label as a functioning link.

## Settings simplification and recovery

The approved Settings direction removes the landing/default-agent controls without
migrating their stored values, moves Archived projects last, and uses text-only
preference choices. Focused Linux checks passed: 18 Settings tests (including
click-to-Space diagnostics, retained launch preferences, archive ordering, nested
navigation and 320px/rem24 refresh bounds), 22 upgrade tests, three environment
refresh tests, 33 source conformance checks, five shared-component checks, five
project-panel tests, five project-setup tests, four action-feedback tests and five
telemetry tests. Strict all-target and production Clippy, formatting, the optimized
production build, 85 reference-comparator checks and repository/tooling checks
passed. Initial disclosure-focus and selector failures were repaired; geometry
source guards were updated to inspect the extracted shared recipe owner.

An isolated Linux X11 Settings fixture uses a separate application identity and
configuration, with synthetic release/provider/error data. Native captures cover
all ten page types, dark/light themes and enlarged-text layouts; the floating
capture matrix measured 900px, 500px and 320px logical widths at display scale 2.
Representative captures of every page type and recovery feedback were visually
inspected. The initial tiled-window run did not honor requested widths and is not
narrow-layout evidence. This is exploratory native inspection, not a certified
measured HTML/GPUI reference. Refresh receipt expiry and stale-work protection are
executor tests; the native captures use reduced motion and do not verify the
animated transition frame by frame.

Codex provider-row bounds and Claude provider-row bounds/diagnostics activation
are verified through GPUI fixtures only. Live Codex and live Claude transport,
permission/launch handoffs, real provider installation, real app-update network
recovery, desktop pointer/keyboard traversal, measured contrast and macOS/Windows
rendering remain untested for this change. Shared project preference-group coverage
is regression-fixture coverage, not an OS-level form walkthrough. No installation
or replacement of the user's application was performed.

## Provider and MCP connection management

Connections introduces account status/sign-in/sign-out, explicit scoped MCP checks and authorization, transient provider interactions, and idle-session reconnect. Source/fixture evidence does not establish production account OAuth or cross-platform callback behavior.

The production-service smoke is `cargo run -p grove-core --example provider_connections_smoke -- PROVIDER EMPTY_CWD status` (or `servers`). Use task-owned provider configuration for MCP probes. Mutation modes `fixture-login SERVER`, `fixture-cancel SERVER` and `fixture-clear SERVER` additionally require `GROVE_CONNECTIONS_ISOLATED_FIXTURE` and isolated provider homes with a controlled localhost OAuth/MCP fixture. They never print URLs/codes/raw output and reject non-loopback authorization URLs. Never use logout probes against the user’s real accounts merely to verify a build.

The ignored Linux native fixture `connections_review_linux_capture` renders the production Settings/Connections surface with test-only backend state. It requires a fresh `GROVE_CONFIG_DIR` and task-owned `GROVE_CONNECTIONS_QA_OUTPUT`; it uses a distinct `dev.grove.connections-review-qa` popup window so tiling does not replace requested narrow dimensions. The optional `GROVE_QA_EXPOSE_HELPER` receives only the exact QA title; after returning, the fixture yields to repaint before capturing. It covers connected, signed-out, external, waiting/input, server failure, sign-out confirmation and context-picker states in light/dark and narrow enlarged-rem layouts, plus scrolled action areas. Verify actual captured state and dimensions: a successful capture command alone does not rule out stale frames on an asleep Linux compositor.

Remaining coverage must be reported separately for Codex and Claude: real user-account browser success/sign-out/expired-session recovery, authentic third-party MCP consent and read-only tool invocation, live native click-to-provider dispatch, and macOS/Windows callbacks, credential storage and cancellation. Localhost OAuth fixtures establish real installed provider/service boundary behavior only for that controlled server; native fixture captures establish the rendered states actually observed, not live account behavior or universal accessibility conformance. No local installation or restart is part of this shipping task.

Linux verification passed 949 core tests (10 manual/live tests ignored), 19 Settings tests and all 33 UI source-conformance checks. The Settings focus regression covers completion from the redirect input, Send and Copy-link actions without stealing unrelated focus. Both production connection adapters completed MCP OAuth, credential clearing, cancellation and fresh status checks against a controlled localhost server. One Claude clearing run encountered the CLI's credential-store error; a separate explicit retry succeeded. Real account status was checked read-only; user credentials were not changed.

The existing side-chat composer-layout assertion failed identically on clean base `1664662` and the feature checkout; no unrelated layout repair was included. Native Linux captures established provider branding, compact actions, connected/signed-out layouts and narrow enlarged-rem wrapping. Waiting/light and sign-out-confirmation/narrow captures still contained prior frames after bounded repaint attempts, so those two visual states remain unverified. GPUI focus/render assertions do not replace that missing screenshot evidence.
