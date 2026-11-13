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

For shared questions, approvals, tasks, subagents, or transcript behavior, name **Codex and Claude** in acceptance checks. Report each as verified, unsupported, or untested with the method or reason. Fixtures do not establish live provider-boundary behavior. Cover affected macOS/Linux/Windows paths when platform behavior changes; do not infer unobserved parity.

## Verify and maintain

Run relevant code checks and the conformance suite as required by [AGENTS.md](../AGENTS.md). A passing source check or worker claim cannot establish native UI correctness. Record the exact build/worktree observed, screenshot/interaction evidence, and passed, failed, or unobserved checks. Correct observed drift; if native inspection is blocked, state the blocker and unobserved checks without claiming verification. An isolated QA app may be used before the final installation; documentation/skill-only changes require neither build nor installation.

Update `product/design.md` when an approved change adds or changes an invariant, and update the UI skill or owning procedure guide when its procedure or checks need to change. Restoring an already documented rule needs no redundant guidance edit. Keep rules semantic.
