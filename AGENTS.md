# AGENTS.md

- For Grove UI changes, HTML reference/mock authoring, screenshot/mock implementation, and visual review, load
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
- Read the [product guidance index](product/README.md) before product changes.
- [product/design.md](product/design.md) is the normative UI contract; the project UI skill
  defines how to implement and verify it. Exact geometry comes from
  `src/views/tokens.rs` (or named module constants for one-consumer dimensions),
  colors from `src/theme.rs`, and control shapes from
  `src/views/components.rs`. Historical CSS catalogs are not authority.
  Update the contract when an approved change adds or changes an invariant,
  and the skill when its procedure or checks change. Fix conformance violations
  in the owning code; do not add exemptions. When an approved contract change
  requires implementation or check changes, update those together; documentation
  corrections alone do not require redundant code changes.

## Repository artifacts

- Put temporary plans, HTML mocks, generated reports, screenshots, browser captures,
  and task output under `.grove-artifacts/` (ignored), or an external temporary
  directory. Configure tools to use that location where supported; default
  `.playwright-cli/` captures are also ignored. Do not force-add scratch output.
- Do not recreate the retired root `docs/`, `mockups/`, `output/`, or `plans/`
  directories or `DESIGN.html`. Maintain agent-facing product/developer guidance under `product/`,
  UI invariants in `product/design.md`, and required fixtures beside their tests.
- Commit files that serve application behavior, tests, supported tooling, or
  maintained documentation. Keep `ui-references/` and `wix/` as supported tooling;
  `.grove/` may contain repository configuration and must not be blanket-ignored.
- Run `python3 scripts/check-repository-artifacts.py` before delivery. CI checks
  tracked paths even when files were added with `git add -f`.
- Retired `.grove/plans/`, `todo.md`, saved temporary-machine paths, and old launch
  media must not return. Write new task plans under `.grove-artifacts/`; maintain
  unresolved verification notes in `product/verification.md` rather than a historical tracker.
- Keep skill entry points short and route them to the relevant `product/` guide.
  Root Markdown is limited to README, AGENTS, CLAUDE and release notes. Legal,
  vendor/font provenance and fixture-local instructions remain with their owners;
  new product documentation belongs under `product/`, not scattered source paths.
