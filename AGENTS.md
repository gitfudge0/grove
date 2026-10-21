# AGENTS.md

- For Grove UI changes, screenshot/mock implementation, and visual review, load
  [grove-ui](.agents/skills/grove-ui/SKILL.md) before editing. It defines the
  native inspection and interaction-state checks that prevent layout drift.
- By default, run `./install.sh` once after the final code changes at the
  selected delivery endpoint. Run relevant code checks before delivery;
  documentation/skill-only changes need no build or installation. Release only
  when explicitly requested. Explicit no-install, stop, cancel, or release
  deferral instructions override the default immediately: propagate the current
  delivery endpoint and instructions to every worker, safely stop affected
  operations owned by this task (not unrelated builds), and do not restart them
  without later authorization. Native UI QA may use a current build in an
  isolated QA app before the final installation; no-install does not require
  changing the user's installed app.
- For changes to shared agent questions, approvals, tasks, subagents, or
  transcript features, name both Codex and Claude in the acceptance checks.
  Report each as verified, unsupported, or untested, with the verification method
  or reason. Mocked fixtures do not establish live provider-boundary behavior;
  identify what was actually exercised. Cover affected platforms when changing
  platform-specific behavior.
- `DESIGN.md` is normative for all UI work. Every numeric value comes from
  `src/views/tokens.rs` and every colour from `src/theme.rs` — never a bare
  literal. Use the shared components in `src/views/components.rs` rather than
  forking a local shape. `cargo test` runs a conformance suite
  (`src/views/conformance.rs`, rules R1–R20) that enforces this; the fix for a
  firing rule is to change the code, never to add an allow-list entry — and an
  allow-list entry that is genuinely sanctioned must name the DESIGN.md clause
  that sanctions it in more than 40 characters of real justification, which
  `every_allow_list_entry_carries_a_justification` enforces. A token with
  exactly one consumer is a module constant, not a scale entry. If a
  design seems to need a token or tier that does not exist, that is a signal
  the design is wrong, per DESIGN.md §13.

- Inputs and dropdowns use neutral fill changes with stable borders for focus
  and open states; never add white or foreground-colored outlines. Action
  buttons do not retain a clicked highlight after release. Keep hover,
  `focus_visible` for keyboard use, intentional selected/open states, input
  carets, labels, and validation error borders.
- The app has one System/Dark/Light appearance setting with fixed palettes. Workspaces and projects do not override it.
- Placeholders contain meaningful example values, never repeated labels or titles (Workspace: `e.g. Platform`). Destructive confirmations start empty and show the exact target name only as the placeholder.
