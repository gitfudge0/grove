# Sidebar restoration tracker

## Status legend and baseline

- **COMPLETE**: present in the redesign; retain and regression-check when touched.
- **IN PROGRESS**: missing or partial Projects behavior in the next authorized phase; implementation/checks remain outstanding unless evidence is recorded.
- **TODO**: approved restoration inventory, not yet implemented.
- Baseline original: `main` at `31e9c00`. Redesign baseline: `b592a01`.
- This inventory records the approved comparison, not a claim that new restoration work has passed tests. Consult the original commit for behavioral details; retain the approved redesign appearance.

## Navigation and views

| Status | Original behavior / current state | Implementation handoff | Targeted tests / visual checks |
|---|---|---|---|
| COMPLETE | Project → worktree → session hierarchy and individual expand/collapse are present. | `src/views/sidebar.rs`, `src/entities/project_tree.rs` | Nested selection, collapse, keyboard reachability. |
| TODO | Global cycle through expanded / sessions-only / collapsed is missing. | Restore tree expansion command/state in sidebar. | Cycle all three states without losing selection. |
| COMPLETE | Projects / Sessions / Grid switching is present. | Sidebar mode state; `src/views/sidebar/grid.rs` | Same selected session/process survives switching. |
| TODO (partial) | Session grouping exists; original priority ordering and rich cards are missing. | Sidebar session rendering and activity data; see Sessions below. | Mixed activity ordering and compact/narrow cards. |
| TODO | Draggable persisted sidebar width and double-click reset are missing. | Shell/sidebar sizing boundary; persist width. | Drag limits, restart retention, reset, narrow window. |
| COMPLETE | Per-workspace navigation state is present. | Sidebar workspace state and `src/settings.rs` | Switch workspaces and return to prior mode/selection. |

## Projects — completed phase

| Status | Original behavior / current state | Implementation handoff | Targeted tests / visual checks |
|---|---|---|---|
| COMPLETE | Add-project source/details setup restored: typed path, suggestions, Browse, name, optional Git initialization and captured workspace. | `sidebar/project_setup.rs`, retained add-project helpers and atomic service registration. | `views::sidebar::project_setup::tests`: 4 tests cover isolated persistence/canonical path/captured workspace, suggestion Up/Down/Tab, Details Escape back and invalid-name draft retention. |
| COMPLETE | Full removal offers optional non-main worktree deletion with discovery, progress and retained final errors. | `sidebar/projects.rs`, path-based service and `ProjectRemovalChanged` subscription. | `project_editor_archive_restore_and_removal_keep_path_identity`: opt-out keeps files; opt-in discovery failure remains visible after unregister; duplicate submission guarded. Successful opt-in deletion/main preservation is implemented by the service but not exercised by this UI test. |
| COMPLETE | Project editor restores rename, setup/run/teardown scripts and archive with live session blockers. Appearance controls were removed so the app theme applies everywhere. | Main-canvas `sidebar/projects.rs`; checked `update_project`. | Isolated editor test verifies rename/scripts, invalid-name/collision draft retention, blocked archive then close/recheck success. |
| COMPLETE | Archived-project list restores original membership/settings or confirms permanent registration removal while keeping files. | Projects-section archive control and path-based main-canvas decisions. | Isolated panel test verifies restore retains scripts, delete cancellation, permanent unregister and disk preservation. Sidebar regression covers workspace-switch panel dismissal, stale-path no-op and resulting project focus. |
| COMPLETE | Finder reveal is present through “Open in file manager.” | Sidebar `Action::Reveal`. | Reveal selected project path, including spaces. |
| COMPLETE | Project counts and non-git marker are present. | Sidebar `project-count-*` and `project-no-git-*`; project tree data. | Counts update with sessions; non-git icon and tooltip; column alignment. |
| COMPLETE | Collapsed-project activity rollup shows highest urgency without moving title/count columns. | Existing sidebar folder indicator, accessible status/tooltip and activity observers. | Priority unit test and GPUI collapse/expand/geometry assertions pass. |

## Worktrees

| Status | Original behavior / current state | Implementation handoff | Targeted tests / visual checks |
|---|---|---|---|
| COMPLETE | Create with name, branch and base is present. | `src/views/sidebar/content.rs`, project service. | Validation, errors, Tab traversal, narrow form. |
| TODO | Delete non-main worktree with confirmation/progress missing. | Restore project-service operation and sidebar action. | Main protection, cancellation, errors, live sessions. |
| TODO (partial) | Codex/Claude/Terminal launch exists; restore full original launch behavior and all installed backends including OpenCode. | Sidebar launch action, `src/launcher.rs`, core agent definitions. | Installed/missing backend states, correct path and backend invocation. |
| TODO | Configured run-script launch missing. | Project scripts and launcher. | Empty script, correct cwd, output/failure. |
| TODO | Collapsed-worktree activity rollup missing. | Project tree and activity store. | Aggregate status updates while collapsed. |
| TODO | Searchable worktree launcher with recents, agent choice and keyboard navigation missing. | Restore launcher UI and command routing. | Filtering, recents persistence, keyboard selection, cancel focus. |
| TODO | Multi-worktree multi-root session launch missing. | Core multi-root domain and launcher. | Root inventory, correct cwd/context, close lifecycle. |

## Sessions

| Status | Original behavior / current state | Implementation handoff | Targeted tests / visual checks |
|---|---|---|---|
| COMPLETE | Select/display terminal, lifecycle status, confirmed close and failed-session retry are present. | Sidebar, content canvas, runtime/session registry. | Selection identity, close cancel/confirm, retry, process lifecycle. |
| TODO (partial) | Title/project/worktree/branch/status identity exists but needs original richness restored. | Sidebar session rows; preserve readable-title fallback. | Long/UUID titles, metadata updates, narrow truncation. |
| TODO | Activity age, multi-root inventory, needs-you/review priority, stable working/idle order and git +/- are missing. | Activity store, registry, project tree and session rendering. | Deterministic ordering, live age/status/counters, multi-root detail. |
| TODO | Diff viewer launched from counters missing. | `src/entities/diff_viewer.rs`; reconnect counter action. | Empty/changed diff, correct repo, close/focus restoration. |

## Standalone terminals

| Status | Original behavior / current state | Implementation handoff | Targeted tests / visual checks |
|---|---|---|---|
| COMPLETE | Add/select/collapse/confirmed close are present. | Sidebar home-terminal actions and registry. | Workspace scope, selection and close cancellation. |
| TODO | Live shell title/cwd and running state need restoration. | Terminal session lifecycle/title data → sidebar rows. | Cwd/title changes, exit, no fabricated running state. |
| TODO | Auto-create a fresh shell after final terminal closes missing. | Home-terminal close lifecycle. | Final close creates exactly one shell in correct workspace; other closes do not. |

## Sidebar-related commands/keyboard

All below are **TODO** restoration items. Inspect original bindings and commands at `31e9c00`; preserve PTY Tab/Ctrl+C and modal focus behavior in the redesign.

| Original behavior / current gap | Implementation handoff | Targeted tests / visual checks |
|---|---|---|
| New-session launcher; scoped worktree launcher. | Launcher + shell/sidebar action registration. | Correct scope, invocation, Escape/focus return. |
| Switch, next, previous and direct session selection. | Shared visible-order navigation. | Boundaries, deleted session, workspace scope. |
| Jump to waiting session. | Activity priority and selection command. | Waiting preserved on focus; deterministic next target. |
| Toggle tree/sessions and Grid. | Existing modes exposed through restored commands. | Toggle return mode and selected process identity. |
| Add terminal command. | Existing terminal action exposed via shortcut. | One creation, correct workspace, terminal focus. |
| Numbered shortcuts aligned with visible order. | One shared ordering source for rows and shortcuts. | Filtering/grouping/collapse changes; every visible number matches target. |

## Relocated settings

**TODO**: replace the disabled settings icon with access to the original settings functions in the new layout. Restore each: one app-wide theme, zoom, backend, permissions, default agent, browser integration, telemetry, archived projects, updates, and shortcuts. Project and workspace theme overrides are intentionally removed.

Implementation: inspect original settings UI, `src/settings.rs`, core persisted settings and shell/sidebar entry point. Reuse existing services; do not invent new settings semantics. Check save/cancel/persistence and disabled/error states for each setting, keyboard access/focus return, narrow layouts, and archived-project flows above.

## Agent continuation protocol

1. Read this file and `git status` first; inspect current code plus the original baseline before implementing an item.
2. Claim specific items/files in this tracker and coordinate overlapping ownership. Treat each COMPLETE row's recorded evidence as the handoff baseline; re-open it if a regression is found.
3. Preserve user/other-agent edits; never revert or discard their work. No commits unless the user asks.
4. Update status, implementation notes, and exact check evidence after each item. Record failures/blockers honestly; mark COMPLETE only after relevant behavior is verified.
5. Run targeted checks only in worker lanes. Root runs full gates, installation and visual inspection. Preserve terminal input routing, workspace scope and process identity throughout.

## Final repository gates — root-owned

- `cargo fmt -p grove -p grove-core -- --check`
- `cargo nextest run --workspace --locked --profile ci`
- `cargo clippy --workspace --all-targets -- -D warnings -A clippy::unwrap_used -A clippy::expect_used`
- `cargo clippy --workspace -- -D warnings`
- `./install.sh`
- Native visual inspection: restored workflows, desktop/narrow windows, all three modes, keyboard/focus behavior, subtle grid borders and terminal padding. Record evidence/results before claiming completion.

### Latest gate evidence (2026-09-23)

- One app theme now applies to app chrome and every terminal. Project/workspace theme controls, saved project pins, and their settings toggle were removed; legacy config keys are accepted on load and omitted on re-save.

- `cargo fmt -p grove -p grove-core -- --check` and `git diff --check`: passed.
- `cargo nextest run --workspace --locked --profile ci`: 884 passed, 2 skipped.
- Both workspace Clippy gates above: passed with warnings denied.
- `./install.sh`: release bundle built, signed and installed to `/Applications/Grove.app`.
- Native Projects inspection passed at desktop and 768px widths for the original flow set; after theme removal, the installed editor was visually checked again and its accessibility tree contains no theme control. Destructive confirmations were cancelled; service behavior is covered by tests.
- Project name and read-only folder now share one field well and 14px UI typography; native editor inspection confirms matching height, inset, fill, border and label/value alignment.
- The approved `screens-project.html` flow canvas has been implemented in the Projects UI with compact headers, centered forms, attached validation, semantic actions and fixed footers.
- Projects remain uncommitted so the next agent can inspect or amend the combined diff before a user-requested commit.
