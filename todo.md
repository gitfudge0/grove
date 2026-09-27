# Sidebar restoration tracker

## Status legend and baseline

- **COMPLETE**: present in the redesign; retain and regression-check when touched.
- **IN PROGRESS**: implementation exists, but a stated check remains outstanding.
- **TODO**: approved restoration inventory, not yet implemented.
- Baseline original: `main` at `31e9c00`. Redesign baseline: `b592a01`.
- This inventory tracks restoration against the original commit while retaining the approved redesign appearance. Check evidence is recorded below; native checks remain open where marked.
- Latest native check attempt: CUA `getApp` timed out both by app name and by `/Applications/Grove.app`; the installed build could not be inspected, so all affected rows remain IN PROGRESS.

## Navigation and views

| Status | Original behavior / current state | Implementation handoff | Targeted tests / visual checks |
|---|---|---|---|
| COMPLETE | Project → worktree → session hierarchy and individual expand/collapse are present. | `src/views/sidebar.rs`, `src/entities/project_tree.rs` | Nested selection, collapse, keyboard reachability. |
| IN PROGRESS (native visual check pending) | Global cycle now steps expanded → sessions-only → collapsed → expanded while retaining selection and per-workspace state. | `src/views/sidebar.rs` tree header control and snapshot-based collapse state. | Three-state grouping, hidden-session selection/process identity, manual toggles, workspace restore and control visibility pass targeted tests; native visual check pending while macOS is locked. |
| COMPLETE | Projects / Sessions / Grid switching is present. | Sidebar mode state; `src/views/sidebar/grid.rs` | Same selected session/process survives switching. |
| IN PROGRESS (native visual check pending) | Sessions view now has four priority sections (Needs you, Review, Working, Idle), stable activity-clock ordering with idle dwell, and compact inset cards. Failed/Starting keep their lifecycle labels and priority. | `src/views/sidebar.rs` session list grouping and cards; see Sessions below for age, multi-root and diff details. | Four targeted ordering/layout tests pass, including 320px geometry; native visual check pending because the desktop window could not be inspected. |
| IN PROGRESS (native visual check pending) | Sidebar width now drags live, persists on release, and resets to the redesign's 260px default on double-click. Narrow windows temporarily cap the displayed width without changing the saved preference. | `src/views/sidebar.rs` divider/width state and `src/views/shell.rs` header alignment; existing `sidebar_width` setting. | Shell interaction tests cover limits, release/reconstruction, reset, zoom, narrow windows, mode switches and header alignment; native visual check pending while macOS is locked. |
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
| IN PROGRESS (native visual check pending) | Delete non-main worktree now has a guarded sidebar action, main-canvas confirmation, teardown/removal progress, skip, and retained errors. | `src/project_service.rs`, `src/runtime.rs`, `src/views/sidebar.rs`, `src/views/sidebar/content.rs`. | Checked Git inventory protects main/unlisted paths and registered project roots inside targets; 16 service tests and 10 sidebar tests cover deletion, aliases/context-root sessions, blocked launches, cancel/focus, duplicate submit, partial errors, and desktop/narrow geometry. Native visual check is pending because macOS locked during inspection. |
| IN PROGRESS (native visual check pending) | Worktree actions now offer Codex, Claude, OpenCode and Terminal. Missing backends are inert and omitted from Tab order; stale worktree paths and failed launches preserve selection. | `src/views/sidebar.rs` launch controls/target guard; existing runtime and core agent invocation. | Sidebar interaction test covers four controls, missing backend, OpenCode path/backend routing, stale path and runtime rejection; core invocation tests pass. Native visual check pending because the Grove window timed out through computer use. |
| IN PROGRESS (native visual check pending) | Configured nonblank run scripts launch from worktree actions as normal selectable sessions. Repeated runs remain separately accessible and visible to close/removal lifecycle. | `src/runtime.rs` validates the project/worktree, runs the script in the worktree cwd, and registers each native session with a distinct ID; `src/views/sidebar.rs` exposes and selects it. | Runtime and sidebar tests cover blank scripts, cwd/output, spawn failure, repeated IDs, session close and retained focus. Native visual check pending. |
| IN PROGRESS (native visual check pending) | Collapsed worktrees show the highest-urgency session activity. | `src/views/sidebar.rs` rolls up live activity in the collapsed row. | Priority test and GPUI test cover status changes, collapse/expand and title/count alignment. Native visual check pending. |
| IN PROGRESS (native visual check pending) | Searchable workspace worktree launcher includes recents, agent choice and keyboard navigation. | `src/views/worktree_launcher.rs`, `src/launcher.rs`, `src/views/shell.rs` and `src/runtime.rs` route selected targets through validated launch and recent-launch persistence. | Launcher tests cover workspace scope, filtering, stale recents/targets, keyboard selection, cancel focus, successful launch, 320x200 layout and alignment with worktree actions. Native visual check pending. |
| IN PROGRESS (native visual check pending) | The launcher now selects multiple worktrees for a supported agent and starts one session in the primary worktree with the remaining roots in its launch context. It preserves the root inventory for retry/close and rejects unsupported or stale selections. | `src/views/worktree_launcher.rs`, `src/runtime.rs`, core multi-root launch arguments and session registry. | Targeted multi-root, cwd/context, stale target and lifecycle tests pass. Native launcher and launched-session inspection is pending because CUA could not reach the installed app. |

## Sessions

| Status | Original behavior / current state | Implementation handoff | Targeted tests / visual checks |
|---|---|---|---|
| COMPLETE | Select/display terminal, lifecycle status, confirmed close and failed-session retry are present. | Sidebar, content canvas, runtime/session registry. | Selection identity, close cancel/confirm, retry, process lifecycle. |
| IN PROGRESS (native visual check pending) | Session rows show a readable OSC title with a label fallback for blank/UUID titles, plus project, worktree, branch and lifecycle/activity status. The Sessions view retains its four priority sections and stable activity-clock order. | `src/views/sidebar.rs` session rows and shared Sessions ordering; registry and activity store. | Title fallback, narrow card geometry and priority/order tests pass. Native title truncation and status presentation remain uninspected. |
| IN PROGRESS (native visual check pending) | Sessions cards show age since the last activity-state change, every selected context root with path fallback when snapshot data is stale, and polled Git +/− counts or a clean/changed state. | `src/views/sidebar.rs`, `src/entities/activity_store.rs`, `src/entities/project_tree.rs`. | Age scale, stale-root inventory, Git state and card geometry tests pass. Live age/counter appearance remains uninspected. |
| IN PROGRESS (native visual check pending) | Changed-file counts open a native diff for the session's own repository, with file selection, refresh, empty/binary/oversize states, Escape/Close and focus return. | `src/views/sidebar/session_diff.rs` and `src/entities/diff_viewer.rs`. | Diff status and repository identity/focus tests pass. Native file/patch layout and changing-file flow remain uninspected. |

## Standalone terminals

| Status | Original behavior / current state | Implementation handoff | Targeted tests / visual checks |
|---|---|---|---|
| COMPLETE | Add/select/collapse/confirmed close are present. | Sidebar home-terminal actions and registry. | Workspace scope, selection and close cancellation. |
| IN PROGRESS (native visual check pending) | Standalone terminal rows show the live shell title and actual Starting/Running/Exited/Failed state. Current cwd is shown only when the shell emits a valid local OSC7; otherwise the row explicitly labels the launch directory. | Terminal session title/OSC7/lifecycle data and `src/views/sidebar.rs` home rows. | OSC7 validation, title/status and launch-directory fallback tests pass. Native shell title/cwd changes and exit presentation remain uninspected. |
| IN PROGRESS (native visual check pending) | Closing the final standalone terminal creates exactly one replacement shell in the same workspace; closing other terminals leaves the remaining shells selected as appropriate. | `src/runtime.rs` home-terminal close lifecycle and sidebar selection. | Final/nonfinal close and workspace identity tests pass. Native close/replace interaction remains uninspected. |

## Sidebar-related commands/keyboard

The commands below are implemented and covered by targeted tests. Native keyboard/focus inspection is still pending, so each remains **IN PROGRESS**. PTY Tab/Ctrl+C and modal focus routing are preserved.

| Original behavior / current gap | Implementation handoff | Targeted tests / visual checks |
|---|---|---|
| New-session launcher and scoped worktree launcher shortcuts. | `src/views/shell.rs` actions open the shared worktree launcher. | Scope, launch, Escape and focus-return tests pass; native check pending. |
| Switch, next, previous, and direct session selection. | Shell actions call sidebar navigation over workspace-visible session order. | Boundaries, removed sessions and workspace scope tests pass; native check pending. |
| Jump to waiting session. | Activity waiting queue and sidebar navigation select a workspace member. | Target and focus tests pass; native check pending. |
| Toggle tree/Sessions and Grid. | Shell actions call sidebar mode toggles with last-mode restoration. | Toggle/selection tests pass; native check pending. |
| Add standalone terminal and close focused session. | Shell actions call sidebar creation/close requests. | Workspace/focus and confirmation tests pass; native check pending. |
| Direct session numbers 1–9 match badges in the currently visible Project tree or priority-ordered Sessions list. Collapsed/filtered rows are skipped and out-of-range numbers do nothing. | Sidebar shares `visible_session_order` between badge rendering and numbered selection. | Ordering, collapse, workspace and direct-number tests pass; native badge/shortcut check pending. |

## Relocated settings

**IN PROGRESS (native visual check pending)**: the sidebar settings control and Settings/Shortcuts commands open the new panel. It exposes one app-wide theme with system/light/dark choices, zoom, backend, permissions, default agent, Claude in Chrome, telemetry, archived projects, updates, and handled shortcuts. Project and workspace theme overrides remain removed.

Implementation: `src/views/settings_panel.rs`, `src/views/shell.rs`, `src/views/sidebar.rs`, `src/settings.rs` and existing services. Targeted panel/shell tests cover persistence, disabled/error states, narrow layout, keyboard focus return and archived-project handoff. Native settings flows still need inspection.

## Agent continuation protocol

1. Read this file and `git status` first; inspect current code plus the original baseline before implementing an item.
2. Claim specific items/files in this tracker and coordinate overlapping ownership. Treat each COMPLETE row's recorded evidence as the handoff baseline; re-open it if a regression is found.
3. Preserve user/other-agent edits; never revert or discard their work. No commits unless the user asks.
4. Update status, implementation notes, and exact check evidence after each item. Record failures/blockers honestly; mark COMPLETE only after relevant behavior is verified.
5. Run targeted checks only in worker lanes. Root runs full gates, installation and visual inspection. Preserve terminal input routing, workspace scope and process identity throughout.

## Final repository gates — root-owned

- `cargo fmt -p grove -p grove-core -p grove-terminal -- --check`
- `git diff --check`
- `cargo nextest run --workspace --locked --profile ci --status-level fail --retries 0`
- `cargo clippy --workspace --all-targets -- -D warnings -A clippy::unwrap_used -A clippy::expect_used`
- `cargo clippy --workspace -- -D warnings`
- `./install.sh`
- Native visual inspection: restored workflows, desktop/narrow windows, all three modes, keyboard/focus behavior, subtle grid borders and terminal padding. Record evidence/results before claiming completion.

### Latest gate evidence (2026-09-23)

- Restoration items 1–4 (2026-09-23): root `cargo fmt -p grove -p grove-core -p grove-terminal -- --check` and `git diff --check` passed. `cargo nextest run --workspace --locked --profile ci --status-level fail --retries 0` finished with 954 passed and 2 skipped. Both workspace Clippy gates listed above passed. `./install.sh` built, signed and installed `/Applications/Grove.app`. Native visual verification is pending: CUA `getApp` timed out when called by name and by `/Applications/Grove.app`.

- Worktrees next three items (2026-09-23): root `cargo fmt -p grove -p grove-core -- --check` and `git diff --check` passed; `cargo nextest run --workspace --locked --profile ci --status-level fail` finished with 917 passed and 2 skipped; both listed Clippy gates passed. `./install.sh` built, signed and installed `/Applications/Grove.app`. Native visual check remains pending because CUA `getApp` timed out twice.

- Worktree backend launch (2026-09-23): focused interaction test passed; `cargo fmt -p grove -p grove-core -- --check` and `git diff --check` passed. Workspace Nextest passed (903 tests, 2 skipped); both workspace Clippy gates passed after a format-string fix. `./install.sh` built, signed, and installed `/Applications/Grove.app`. Native visual inspection remains pending because the Grove window timed out through computer use.

- Sidebar width (2026-09-23): focused Shell tests passed (9), including drag limits, persistence, reconstruction, double-click reset, zoom and narrow layout; `cargo fmt -p grove -p grove-core -- --check` and `git diff --check` passed. Workspace Nextest passed (902 tests, 2 skipped); both workspace Clippy gates passed. `./install.sh` built, signed, and installed `/Applications/Grove.app`. Native visual inspection remains pending because macOS is locked and automatic unlock failed.

- Sessions list priority/cards (2026-09-23): `cargo test --locked views::sidebar::tests::sessions_list_` passed (4 tests); `cargo fmt -p grove -p grove-core -- --check` and `git diff --check` passed. Workspace Nextest passed (899 tests, 2 skipped); both workspace Clippy gates passed. `./install.sh` built, signed, and installed `/Applications/Grove.app`. Native visual inspection remains pending: the Grove desktop window timed out through computer use.

- Tree expansion cycle (2026-09-23): targeted `cargo test --locked tree_expand -- --nocapture` passed (7 tests). `cargo fmt -p grove -p grove-core -- --check` and `git diff --check` passed. Workspace Nextest passed (895 tests, 2 skipped); both workspace Clippy gates passed. `./install.sh` built, signed, and installed `/Applications/Grove.app`. Native visual inspection remains pending because macOS is locked and automatic unlock failed.

- One app theme now applies to app chrome and every terminal. Project/workspace theme controls, saved project pins, and their settings toggle were removed; legacy config keys are accepted on load and omitted on re-save.

- `cargo fmt -p grove -p grove-core -- --check` and `git diff --check`: passed.
- `cargo nextest run --workspace --locked --profile ci`: 884 passed, 2 skipped.
- Both workspace Clippy gates above: passed with warnings denied.
- `./install.sh`: release bundle built, signed and installed to `/Applications/Grove.app`.
- Native Projects inspection passed at desktop and 768px widths for the original flow set; after theme removal, the installed editor was visually checked again and its accessibility tree contains no theme control. Destructive confirmations were cancelled; service behavior is covered by tests.
- Project name and read-only folder now share one field well and 14px UI typography; native editor inspection confirms matching height, inset, fill, border and label/value alignment.
- The approved `screens-project.html` flow canvas has been implemented in the Projects UI with compact headers, centered forms, attached validation, semantic actions and fixed footers.
- Projects and the UI skills were committed and pushed at `924100f`. The worktree-deletion change is included in this branch; its native dialog check remains open before marking the Worktrees row complete.
- Worktree-deletion gates (2026-09-23): format and diff checks passed; 892 workspace tests passed, 2 skipped; both workspace Clippy gates passed; `./install.sh` built, signed, and installed `/Applications/Grove.app`. Native UI inspection remains pending because macOS locked and automatic unlock failed.
- Typography and proximity pass (2026-09-23): shared UI roles increased to 11/12/13/14px, control height to 24px, and related label gaps tightened while project/section gaps increased. Terminal and editable code metrics stay fixed. `cargo fmt --check`, `git diff --check`, `cargo test --locked` (543 passed), workspace Nextest (892 passed, 2 skipped), and both workspace Clippy gates passed; `./install.sh` built, signed, and installed the release app. The running Grove process was not restarted, so the newly installed layout still needs a fresh-process visual check.
