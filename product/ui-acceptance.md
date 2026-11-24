# UI acceptance

Read this guide when implementing or reviewing Grove UI, and before reporting
completion. Select checks for the actual target established by the
[UI skill](../.agents/skills/grove-ui/SKILL.md); a pass on a similarly named
component is not evidence for the requested surface.

## Match evidence to the outcome

For each affected acceptance outcome, identify an observation capable of detecting
its failure. Record the actual result and method in existing task state; no
separate report or approval is required. Source checks establish source properties,
native captures establish appearance for their captured state, native input tests
establish the interactions actually exercised, and live transport checks establish
provider-boundary behavior. One method does not substitute for another.

Inspect the exact build and target view delivered, including the surrounding
labels that distinguish it from similar controls. Preserve previous valid evidence
for unchanged behavior, but do not attribute it to a newer build without checking
that the relevant inputs are unchanged. When inspection is unavailable, complete
independent authorized work and leave that acceptance outcome explicitly unverified.

For performance changes, define representative short and long sessions with the
mixed built-in components and interactions implicated by the request. Compare
matching workloads and builds. Keep CPU replay timings separate from presented
frame times, scrolling and input latency; do not claim smoothness or short/long
parity from CPU improvements alone. Record the original experience target as
unfinished when only an intermediate metric improves.

## Verify an approved design in the app

Start from the [approved design handoff](ui-mocks.md#approved-design-handoff).
As soon as one representative screen renders through its native route, compare
it with the pinned reference. Make this comparison before extending the shared
layout or component to other screens so a mistaken interpretation stays local.
Repeat the comparison on the exact delivery build for the affected states.

Match viewport, theme, zoom, fixture content, and interaction state. Include the
surrounding pane and edges that establish the approved placement. For example,
show the workspace edge when checking the Settings rail and show the removal
details when checking where their buttons land. Record the build, conditions,
paired captures, and results in task state.

Where existing native geometry tests can cover a stable layout relationship,
derive the assertion from the approved expectation. A Settings rail anchored to
the workspace edge should share the content pane's left edge. Correct any drift
and rerun the affected comparison. If a capture is unavailable, leave visual
verification open and record the blocker separately from functional checks.

## Choose the relevant checks

Inspect the actual native rendering against the selected reference at normal and narrower widths, with long names where applicable. Check spacing, type, contrast, glyphs, clipping, and stable columns. Audit action and navigation buttons for a visible catalog icon, including custom controls, app-owned HTML actions and disabled/loading states. Text-labelled grouped preference choices use labels and neutral selection without decorative icons; icon-only grouped controls retain catalog glyphs and accessible names. Distinguish native radio/checkbox/switch controls and text links by their actual semantics; text-only action and navigation buttons are violations. For text-and-icon buttons, including directional and custom actions: the icon must precede the label with the shared `SPACE_SM` gap. Measure rendered glyph/label bounds at default and changed rem zoom; disabled and loading states retain their leading slot. Distinguish buttons from non-button field indicators and metadata before moving trailing glyphs. Exercise the states changed by the task rather than every screen on every edit:

| Change | Native acceptance checks |
|---|---|
| Controls/forms/menus | Rest, hover, keyboard focus, selected/open, invalid, disabled, loading, and release; stable input/dropdown borders, component-owned keyboard focus, and reserved action slots; selectable readonly values; accessible names. |
| Navigation/checkout/diff | Long titles yield to fixed actions/counts; tree/list/compact treatments remain distinct; narrow Checkout disclosure supports focus/Escape; loading stays in its row; destination and checkout identity remain correct. |
| Tasks/composer/transcript | Expand/collapse, browse and compaction preserve preferences; minimap stays pane-anchored; session switches retain ownership; response Copy includes collapsed prose; hover/focus controls do not shift layout; rewind returns the selected message to the composer without sending, saved-draft swaps preserve text/images/references, and pending attachment work cannot cross a draft handoff. |
| Source-attached confirmation | Actual trigger anchor, pane bounds, blocked background/HTML/scroll, contained Tab, Escape/Cancel focus restoration, incoming-update stability, message/thread identity. |
| Settings | Scroll and category reset, reachable navigation, nested Back/Escape source focus, contained Tab, correct switch meaning. |
| Native plans/reports/HTML | Plan section above Agents, latest revision per plan, row-to-peek pointer crossing/pinning, keyboard/Escape source focus, complete inline Markdown remains visible while the latest-revision Markdown peek is open, read-only peek and contextual decisions in inline/expanded views, custom answers through the existing composer with saved draft restoration and pending-save locking, workspace navigation retained on expansion, footer outside body scrolling at narrow/wide widths and zoom, explicit decision update versus finalized-revision approval with truthful unavailable/delivery states, exact update/approval payload and revision provenance, inert multiline authored text, linked HTML/return and external links, integrated-report theme/zoom, and motion updates without lost input/document state. |
| Motion/keyboard routing | Native reduced-motion static endpoints without scheduled animation frames; HTML CSS/Web Animation suppression and cooperating scripts; type spaces into affected inputs; relevant shortcuts route to the owner. |

For transcript code-layout caching, compare warmed and fresh code geometry after
narrowing and widening the pane, including Unicode carets, syntax styling and rem
changes. Exercise offscreen Copy, Run, and the command directory control, including
keyboard activation, as well as expanded tool Copy. Open an editor or run output and change the execution directory to check
that mutable footers reflow. Include a shell fence nested in a Markdown list
followed by prose. Check that source replacement and document removal release
cached layout. Keep these native/structural checks separate from live
provider streaming and compositor presentation measurements.

For shared questions, approvals, tasks, subagents, or transcript behavior, name **Codex and Claude** in acceptance checks. Report each as verified, unsupported, or untested with the method or reason. Fixtures do not establish live provider-boundary behavior. Cover affected macOS/Linux/Windows paths when platform behavior changes; do not infer unobserved parity.

## Activity design rubric

Use this rubric before selecting a Dashboard Activity design and again when
reviewing the native implementation. Start with a short set of real or
sanitized session examples. For each, write down what changed, what still
needs attention, and which action the session state actually permits. Try the
copy on its own before choosing a layout. Apply the
[design contract](design.md#dashboard-session-launch) for visual and interaction
rules.

| Check | Pass when | Look for |
|---|---|---|
| **Insight at a glance** | With zero clicks, a reader can identify the session and explain the latest outcome, current work, or decision it needs. Any verification gap that affects the next decision is visible. Quiet sessions can stay brief. | Status words repeated as summaries, old task completion presented as progress on the latest request, vague success claims, and the key caveat clipped at narrow widths. |
| **Direct, clear action** | The relevant next action is discoverable where the insight appears. A straightforward action starts in one activation where the current state permits it. A consequential action opens the exact request and its context in one activation, with its existing confirmation separate. Keyboard users can find and activate the same controls. | Generic links with indistinguishable destinations, extra navigation to reach routine actions, controls that overstate what they do, and lost focus or place on return. |
| **Triage across sessions** | A reader can find the session that needs them, distinguish pending verification from work in progress, and reach the relevant output among several sessions. Opening detail or acknowledging a result leaves the next item easy to find. | One long session dominating the feed, competing action emphasis, accidental selection, and scroll or focus jumps while switching items. |
| **Faithful to Grove** | The design uses the existing type, spacing, status marks, catalog icons, and control behavior. Visible emphasis follows what needs attention. Session identity and action ownership stay clear at normal and narrow widths. | Decorative panels or badges that add reading work, provider status inferred from appearance, and controls changing meaning or position without clear feedback. |

Compare the candidate with current Grove using the same session examples.
Include an active request, a review with an unverified item, a working and a
quiet session, and a long conversation whose checklist predates its latest
result. Ask the reviewer to find what needs a decision, say what remains
unverified, and open the relevant output. Record the first choice, activations
to the intended action, and any hesitation or wrong turns. Walk the relevant
paths at normal and narrow widths with pointer and keyboard.

Mark each check **Pass**, **Needs revision**, or **Unverified** and cite the
observation, build or mock, and input method. An expert walkthrough records the
reviewer's observations. Claims about reduced cognitive load require evidence
from people doing the task. For changes touching shared provider behavior,
report Codex and Claude separately under the evidence rules above.

## Verify and maintain

Run relevant code checks and the conformance suite as required by [AGENTS.md](../AGENTS.md). A passing source check or worker claim cannot establish native UI correctness. Record the exact build/worktree observed, screenshot/interaction evidence, and passed, failed, or unobserved checks. Correct observed drift; if native inspection is blocked, state the blocker and unobserved checks without claiming verification. An isolated QA app may be used before the final installation; documentation/skill-only changes require neither build nor installation.

Update `product/design.md` when an approved change adds or changes an invariant, and update the UI skill or owning procedure guide when its procedure or checks need to change. Restoring an already documented rule needs no redundant guidance edit. Keep rules semantic.
