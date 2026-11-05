# Button icon audit — 2026-11-05

The approved rule is now in DESIGN.md: every button contains a visible catalog icon. Text buttons use a leading icon and SPACE_SM; disabled buttons retain it, and loading occupies the same slot. Button-based preferences are included. Actual radio buttons, checkboxes, switches, links, fields and metadata retain their own semantics.

This document preserves the original source audit and records its implementation below. All 22 groups in the table were remediated. Locations in the original table refer to the pre-implementation source; repeated and dynamic controls are grouped by owning construction rather than counted as individual runtime buttons. Native evidence remains limited to the explicitly captured surfaces.

## Originally confirmed missing icons — remediated

| Surface | Source location | Missing-icon controls | Suggested catalog meaning |
|---|---|---|---|
| Plan-ready card | `src/views/codex_view.rs:2843` | Review response | edit |
| Backend setup | `src/views/shell.rs:1010` | Native and Tmux choice cards have Button roles and text only | terminal or state mark |
| Backend setup | `src/views/shell.rs:1118`, `:1132` | Back; Start session | arrow-left; play |
| Session recovery | `src/views/sidebar/content.rs:1311` | Focus terminal | terminal |
| Side chat | `src/views/sidebar/side_chat.rs:986`, `:1400` | Checkout recovery path buttons in both recovery presentations | folder or restore |
| Side chat | `src/views/sidebar/side_chat.rs:1370` | Retry, including disabled unsupported state | refresh |
| File viewer | `src/views/file_browser.rs:838` | Source / Preview toggle | code / eye |
| Legacy plans | `src/views/native_plan.rs:881` | Open saved HTML attachment | external-link |
| Compact Checkout | `src/views/sidebar/checkout_panel.rs:3303` | Zen-mode Plan / Agents / Project tools / Checkout summary button | contextual destination or disclosure |
| New session | `src/views/codex_view.rs:1676` | Starter prompt cards (shared header frame supplies no icon) | meaning appropriate to starter |
| Composer | `src/views/codex_view.rs:7540` | Standard / Fast / Ultrafast speed buttons, including unavailable state | speed or meaningful state |
| Composer suggestions | `src/views/codex_view.rs:8506` | Non-file completion rows; icon is added only when `file_rows` is true | command / skill identity |
| Review summary | `src/views/review_summary.rs:434` | Expand/collapse prose button; body contains text only | disclosure chevron |
| Settings | `src/views/settings_panel.rs:325` | All / Light / Dark theme filters | meaningful filter/theme identity |
| Settings | `src/views/settings_panel.rs:1117` | Percentage / Reset zoom | reset |
| Settings | `src/views/settings_panel.rs:1148` | Default landing-page buttons | destination |
| Settings | `src/views/settings_panel.rs:1220`, `:1230` | Built-in / Tmux backend | terminal |
| Settings | `src/views/settings_panel.rs:1253`, `:1263` | Claude in Chrome and Run agents in terminal Off / On | state mark in both states |
| Settings | `src/views/settings_panel.rs:1342` | Default-agent choices, including unavailable agents | provider identity |
| Settings navigation | `src/views/settings_panel.rs:1813` | Category buttons | category identity |
| Script editor | `src/views/sidebar/projects.rs:665` | Shell / Prompt type buttons | terminal / reply |
| Shared boolean preferences | `src/views/components.rs:1124` | Off / On buttons contain only text; helper declares Button role | meaningful state icon for each value |

The shared boolean finding affects Reduce motion (`settings_panel.rs:1128`), Yolo mode (`:1241`), telemetry (`:1378`), Initialize Git repository (`sidebar/project_setup.rs:574`), Open destination workspace after moving (`sidebar/projects.rs:1401`), and removal's worktree preference (`:1528`). Selected fill does not satisfy the icon requirement.

Suggested meanings are remediation guidance, not a verified inventory of available icon asset names.

## Coverage and remaining checks

Searched all `src` for direct Button construction, click handlers and explicit Button roles (317 matching lines, including duplicate hits for the same control and test fixtures). Inspected shared `form_action`, `transcript_action`, `dense_action_with_colors`, `settings_control`, `boolean_choices`, header and disclosure frames and their consumers. The sole direct `Button::new` is the shared dense helper; it supplies a loading icon but no default rest icon. `settings_control` and header/disclosure frames also permit iconless content, so their consumers need inspection rather than assuming helper compliance.

The sweep included shell, launcher, workspace management, Settings, sidebar navigation/project flows, Checkout, history, composer, transcript, questions, approvals, native plans, inline HTML chrome, file viewer, diagrams, release highlights, tasks, activity and previews. Existing header actions and most dense action consumers explicitly supply icons. Task rows may supply shared status marks inside their child composition; a nearby-text search alone is not enough to classify these as missing. Native radio/checkbox choices and popup menu items were distinguished from action buttons; menu items are not counted above. Ordinary authored HTML and archived mock/reference documents are outside the app-owned button inventory. No literal HTML button definitions were found under `src` or `assets` in the scoped search.

Secondary conformance issue observed: the Settings theme chooser has a trailing chevron after its label (`settings_panel.rs:1098`); it has an icon, but does not meet the existing leading-icon rule. This is separate from missing-icon findings.

## Implementation and verification

All listed findings were repaired using existing catalog meanings plus four new catalog glyphs: `toggle-on`, `toggle-off`, `sun`, and `moon`. The Settings theme chooser now leads with the contrast glyph. New-session starters use search/edit/diff, speed choices use lightning, and non-file completion rows use the neutral tool glyph beside their two-line text. Review summaries use a leading disclosure chevron. Additional source inspection found and repaired project/checkout breadcrumb buttons and minimap message buttons. Minimap rows now reserve the full ICON_SM glyph height and retain independent rail scrolling; its geometry assertion was updated from the previous SPACE_LG bar height. These static identity additions add no motion; existing state and entrance/exit behavior remains owned by the original views.

Added a conformance guard scanning bound dense actions across native view sources for a leading vendor icon slot. It complements custom-control inspection rather than certifying every dynamic runtime branch. Shared Off/On icons are present for both values regardless of selection or enabled state; existing disabled/loading dense geometry checks exercise the leading slot.

Passed checks:

- 33 design conformance tests, including catalog assets and the new app-wide bound dense-action guard.
- 16 active Settings tests (keyboard category navigation, focus containment/restoration, save behavior, narrow/zoom geometry and scroll reachability); two opt-in native captures are skipped by this group, with the new button capture run separately below.
- 14 composer/minimap/review tests covering starter keyboard activation and narrow wrapping, speed keyboard and unavailable-state handling, narrow/zoom speed placement, completion persistence, review parsing/links and minimap click/anchoring/overflow behavior.
- 14 additional file viewer/native-plan tests, including narrow file-header bounds and synthetic Codex and Claude review/edit/send routing.
- Leading icon/label geometry test at default/changed rem size and enabled/disabled/loading states.
- Native Linux X11 Settings button capture with bundled fonts, inspected in Grove Dark and Grove Light at 16px rem and Grove Dark at 24px rem: `/tmp/grove-settings-buttons-grove-dark-16.png`, `/tmp/grove-settings-buttons-grove-light-16.png`, `/tmp/grove-settings-buttons-grove-dark-24.png`. Leading glyphs and labels were visible without overlap in observed content. At larger rem the body reflows and scrolls; the category strip retains its existing scrolling behavior. Sampled supporting foreground/background contrast in the preference area was 8.18:1 dark and 5.70:1 light; this does not certify every state or surface.

One intermediate compile attempt used an unavailable regex dependency in the new source guard; it was replaced with the existing constructor parser and standard string scanning. The initial minimap run exposed the old height assertion; all five minimap tests passed after updating the assertion for its new full-height glyph slot.

**Codex: live provider boundary untested. Claude: live provider boundary untested.** The passing tests exercised synthetic native UI/session fixtures, including shared provider Settings rows and review artifacts for both providers; no live agent was launched. Real platform rendering was inspected for Linux Settings only. Other modified screens have source/fixture evidence, not native raster inspection; macOS/Windows rendering, exhaustive pointer/keyboard state combinations, disabled/loading native screenshots, and live provider speed availability remain untested.

`cargo fmt --all -- --check` and `git diff --check` passed. The final `./install.sh` local build and Debian bundle completed, but the fallback copy encountered a destination collision during another concurrent installation (`File exists`). Delivery was then verified directly: the executable at `/home/gitfudge/.local/bin/grove` exactly matches `target/local/grove` (SHA-256 `06e21bb80be00b06752b63240954a13e4e2f2c115b314aa4fa725539a95fd803`); its launcher and icon exist. The script did not exit successfully, but the final built artifact is installed. No release was published.

Unrelated concurrent edits in the checkpoint/provider core and vendored text renderer were left intact. The isolated native captures above used the test build in this checkout, not a live provider session.
