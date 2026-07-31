# gpui Rewrite Plan 09: Upgrade flow, telemetry, quit paths, persistence

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. This is **production** code: the workspace clippy denies apply (`unwrap_used`/`expect_used`), superpowers:test-driven-development governs every pure helper (tests before implementation, red before green), and superpowers:verification-before-completion governs every "done" claim — read raw command output, never a summary line. Also load the `gpui-development` skill before writing any gpui code; training-data gpui is stale and this rev is pinned.

**Goal:** grove-gpui has every screen and every modal, and none of the machinery that lives *around* them. This is the phase that collects the eight in-code markers Plans 03–08 left behind — `main.rs:72` (close-request interception), `app.rs:111` (telemetry + panic hook), `settings.rs:10,68-83` (`flush_now`/`is_dirty` with no caller), `views/workspace.rs:891` (`flush_ui_zoom_save` on quit) and `:1380` (the cog's upgrade dot stubbed `false`), `views/appbar.rs:84,217` (the same dot), `views/modals/settings.rs:224,668,685` and `views/modals/mod.rs:343` (the Updating/changelog shells that "render whatever `UpgradeState` reports", where `UpgradeState` does not yet exist) — and turns them into the four systems spec Appendix A's **System** paragraph names: the upgrade flow end to end, telemetry, the quit/exit paths with their persistence flushes, and tmux sidecar discovery/reattach.

It also owns two gaps the master plan handed forward:
- the **`spawn_native` agent-invocation gap** recorded by Plan 06 (master row 06: "the native fallback spawns a bare login shell and never invokes the agent … this gap still needs an owner"). Task 6 takes it, and Global Constraint "Carried decision 5" records the scope call and why it is a port and not an accepted limitation.
- the **cog upgrade dot**, stubbed `false` since Plan 06 (`views/appbar.rs:84`), which becomes real the moment `UpgradeState` does.

Exit gate (master plan row 09): **System checklist rows green** (spec Appendix A → the *System* paragraph, enumerated verbatim as rows 1–10 in Task 7 Step 2); `./install.sh` green; one commit.

**Out of scope — do not build it here.** The scripted screenshot sweep across every screen/modal × 3 zooms × 4 themes and the **idle-power measurement** (spec §9 spike 5 — System checklist row 9's *measurement* half; its *mechanism* is already shipped and is only eyeballed here) → Plan 10. The macOS dock badge/bounce sign-off and the mac-only ⌘ chord behaviors → Plan 10 on a macOS host. Deleting the iced app and vt100 → Plan 10. IME composition and the Wayland clipboard round-trip (findings amendment 4 / §S4 Deviation 4) → Plan 10's manual sweep. **The Settings modal's Tools section** (`src/gui/view/modals/settings.rs:400-430`, `detect_tools_task` at `src/gui/update/upgrade.rs:158-191`) has **no counterpart in grove-gpui** — see "Recorded ambiguities" 6; it is reported, not silently absorbed. Do not touch the terminal element, the grid math, the 480ms activity task, or any modal view beyond the upgrade shells and the two Settings rows named in Task 4.

**Architecture (new/changed files only):**

```
crates/grove-gpui/
  src/telemetry.rs              NEW (Task 1). Port of `src/telemetry.rs`:
                                the compiled-in PostHog key, the opt-out
                                gates, the per-install id, `scrub_paths`,
                                `track`/`track_blocking`, the heartbeat.
                                No gpui types; `scrub_token` is TDD'd with
                                the four ported tests.
  src/entities/upgrade_state.rs NEW (Task 3). The PURE upgrade machine:
                                `UpgradeState`, `ChangelogState`, the
                                check-due predicate, the manual-vs-silent
                                error policy, the apply-progress drain.
                                No gpui types; all TDD.
  src/entities/upgrade.rs       NEW (Task 4). The `Upgrade` entity: the
                                gpui half — background `latest()`/
                                `releases()`/`apply()` on the executor,
                                the 3s launch check, the 24h timer, the
                                refocus check, restart.
  src/reattach.rs               NEW (Task 5). The PURE reconciliation:
                                discovered tmux sessions × the registry's
                                current metas -> what to attach, in what
                                order. No gpui types; all TDD.
  src/main.rs                   MODIFIED: `on_window_should_close`, the
                                panic hook, the quit flush.
  src/app.rs                    MODIFIED: telemetry init + `app_launched`
                                + heartbeat (the `app.rs:111` marker).
  src/settings.rs               MODIFIED: `flush_now` gets its callers;
                                `telemetry_enabled()` accessor.
  src/entities/session_registry.rs
                                MODIFIED: `SpawnTarget` carries the agent's
                                launch args; a reattach constructor.
  src/entities/terminal_session.rs
                                MODIFIED: `spawn_native` invokes the agent
                                (Task 6); `attach_existing` (Task 5).
  src/views/workspace.rs        MODIFIED: hosts the `Upgrade` entity, drains
                                the pending grid-order persist, the quit
                                flush, the real `upgrade_available`.
  src/views/modals/settings.rs  MODIFIED: the Updating/changelog shells stop
                                being shells; `CheckUpdates` runs a check.
  src/views/appbar.rs           MODIFIED: nothing — it already reads
                                `AppbarCtx::upgrade_available`; only the
                                value feeding it changes (workspace.rs:1380).
```

**Tech stack additions: none.** `ureq` is already a `[workspace.dependencies]` entry consumed by `grove-core` (`Cargo.toml:96`), and **every network call this phase makes goes through `grove_core::upgrade`**, which owns the only HTTP client in the product (`crates/grove-core/src/upgrade.rs:253-277,418-447,639-654`). Telemetry's PostHog POST is the one exception and it ports its own three-line `ureq::AgentBuilder` verbatim from `src/telemetry.rs:155-163` — `ureq` therefore becomes a direct dependency of `crates/grove-gpui` (`ureq.workspace = true`, same version/features, no new crate in `Cargo.lock`). **If anything in this phase seems to need a different HTTP client, STOP and report.** gpui/alacritty/gpui-component pins unchanged.

## Global Constraints

- Branch: `gpui-rewrite`. Toolchain regime is **identical to Plans 03–08** and is not re-litigated:
  - grove-gpui builds/tests/clippy only via `PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 -p grove-gpui`.
  - Bare `cargo build` / `cargo test` (default-members, rustc 1.94.1) must keep working untouched for `grove`, `grove-core`, `grove-terminal`. Never run `--workspace`.
  - clippy for grove-gpui runs **`--no-deps`**: `cargo +1.95.0 clippy -p grove-gpui --all-targets --no-deps -- -D warnings`.
  - `rustfmt --edition 2021` on **touched files only**, and **never** on anything under `vendor/`.
- **Pins are law.** ZED_REV `1a246efd7e1b83ab568ec5e3e6c1a43a42e1abba`; alacritty fork `4c129667ce56611becdc82de6e28218c80e2e88f`; GPUI_COMPONENT_REV `88f102d13654fe25aa2fede076274b6b751a3704` (vendored, frozen).
- **Constraint 3 — grove-core and the iced app are read-only.** No edits under `src/`, `crates/grove-core/`, or `crates/grove-terminal/`. Amendment protocol unchanged: if a genuinely **UI-free** helper must be exposed from grove-core, **STOP and report**; the orchestrator authorizes it. Do not edit grove-core on your own judgement.
  - **Expected outcome this phase: no amendment is needed, and this was checked function by function.** `grove_core::upgrade` is already a complete, UI-free, fully public module — `detect`, `latest`, `releases`, `apply`, `update_available`, `clean_markdown`, `InstallMethod`, `Release`, `ReleaseNote`, `Stage`, `UpgradeError` (`crates/grove-core/src/upgrade.rs:106-147,164-172,238-297,586-700`); its doc comment already says "the gui layer orchestrates it". Reattach is covered by `tmux::{list_grove_sessions, has_session, pane_pid, configure_embedded_session, SOCKET}` (`crates/grove-core/src/tmux.rs:296-311,87,…`) and `session_meta::{read, prune}` (`crates/grove-core/src/session_meta.rs:42-84`). The `spawn_native` fix is covered by `Agent::{invocation, launch_args, program}` (`crates/grove-core/src/agent.rs:63-82,93-122`). Persistence is `storage::{save, persist}` and `Store`, already consumed. Telemetry needs only `storage::config_dir()` (`src/telemetry.rs:69-74` routes through it precisely so `GROVE_CONFIG_DIR` is honored) — already public.
  - **Foreseen candidate spots** (report, do not act): (1) `grove_core::session::Session` — still forbidden; it owns the vt100 parser, and the reattach path must go through `TerminalSession`, never `Session::attach_existing`. (2) `src/telemetry.rs` lives in the **iced crate**, not grove-core — it is read-only-as-oracle. Reimplement it in grove-gpui with its tests, exactly as Plans 05/07/08 did for iced-side pure logic. Do **not** propose hoisting it into grove-core to share it; iced is deleted in Plan 10 and a two-month-old shared module is not worth an amendment.
- **Behavior questions are answered by reading the iced code, never by guessing.** Canonical oracles for this phase, cited per task:
  - **Upgrade.** `crates/grove-core/src/upgrade.rs` — the whole engine (read `latest` :253-277, `releases` :639-654, `apply` :281-297 and the `Stage` callback contract :140-147,281-285 before Task 3). `src/gui/update/upgrade.rs` — the **orchestration authority**: `on_upgrade` dispatch (:9-24), `on_check_for_updates` with its in-flight guard (:26-32), `on_update_check_result` (:34-63 — records `last_update_check` **regardless of outcome**, then manual-vs-silent error policy), `on_skip_version` (:65-75), `on_copy_release_url` (:77-82), `on_start_update` (:84-113 — the background thread, the `Arc<Mutex<UpgradeProgress>>` handoff, the `update_applied` event), `on_restart_app` (:115-125 — relaunch, **then `flush_ui_zoom_save`, then `exit(0)`**, in that order), `on_open_changelog` (:127-133), `on_changelog_loaded` (:135-143), `on_close_changelog` (:145-149), `maybe_check_updates_due` (:197-207 — the 24h rule *and* the "None means don't double-fire at boot" rule), `check_updates_task` (:213-220), `fetch_changelog_task` (:224-230, limit 10).
  - `src/gui/mod.rs:56-68` — the **3s-delayed** launch check (and the seed-appearance task beside it, already ported at `crates/grove-gpui/src/main.rs:87-92`). `src/gui/update/tick.rs:78-101` — the apply-progress drain and the periodic-check trigger. `src/gui/state.rs:193-201,434-458,810-826` — `UpgradeState`, `ChangelogState`, `UpgradeProgress`, `UpgradeMsg`. `src/gui/view/appbar.rs:29` — the cog's green dot condition (`matches!(self.upgrade, UpgradeState::Available(_))`). `src/gui/view/modals/upgrade.rs:16-97` (the Updating/available modal) and `:98-182` (the changelog).
  - **Telemetry.** `src/telemetry.rs` (whole file — the port target) and `src/main.rs:11-30` (the **panic hook**: message stays local, only the scrubbed *location* is transmitted, `track_blocking` because a spawned thread may not run before exit, and the previous hook is chained). Call sites, all of them: `src/gui/update/mod.rs:102-118` (`set_enabled` from the store, then `app_launched` with theme/project_count/tmux_enabled, then `start_heartbeat`), `:537,542,547` (`zoom_changed` ×3), `:552` (`settings_opened`), `:1029` (`tile_moved`); `src/gui/update/palette.rs:15` (`launcher_opened`); `src/gui/update/sessions.rs:261-269` (`session_ended` with agent/duration_min/tmux), `:463-472` (`session_created` with agent/tmux/open_sessions/open_native/open_tmux), `:480` (`error` kind=spawn_failed); `src/app/spawn.rs:221` (`error` kind=worktree_failed), `:229` (`worktree_created`); `src/gui/update/upgrade.rs:69-72` (`update_declined`), `:103-106` (`update_applied`); `src/app/mod.rs:331-344` (`telemetry_enabled()` / `set_telemetry_enabled`).
  - **Quit paths.** `src/gui/mod.rs:86` — `.exit_on_close_request(false)`, the reason a close-request handler exists at all. `src/gui/update/mod.rs:384` — the `close_requests()` subscription; `:592` — its dispatch. `src/gui/update/modals.rs:338-362` — `on_close_requested`: **tmux-backed sessions do not block a quit, only running native ones do**; zero native → `flush_ui_zoom_save()` then close; otherwise the quit confirm with its singular/plural noun and the documented clobber gap. `src/app/mod.rs:275` — `native_sessions_running`. `src/gui/update/modals.rs:558-576` — `submit_modal_confirm`, where `ConfirmKind::Quit` reaches `iced::exit()` (:569).
  - **Persistence.** `src/gui/update/layout.rs:495-517` (`set_ui_zoom` — clamp, snap to 0.1, early-return on no-op, then **arm** the debounce) and `:518-529` (`flush_ui_zoom_save` — a no-op unless armed; its doc comment lists all three terminating paths). `src/gui/update/mod.rs:52-56` (`ZOOM_SAVE_QUIET_TICKS = 4`, ≈250ms) and `src/gui/update/tick.rs:18-26` (the countdown). `src/gui/update/layout.rs:478-481` (`persist_sidebar_width`), `:486-489` (`persist_grid_order`, mapped through the stable session key). `crates/grove-core/src/storage.rs:100` (`ui_zoom`), and the `storage::save` / `storage::persist` distinction as every existing call site uses it.
  - **Reattach.** `src/app/util.rs:12-20` (`discover_sessions` — list, filter by `has_session`, `attach_existing`, **silently drop failures**), `src/app/mod.rs:219-224` (startup: **only when `tmux_available`**, and existing tmux sessions keep their backend even if the saved preference now says native), `:347-366` (`discover_tmux_sessions` — the *re-scan* after tmux is switched on, deduplicated by `tmux_name`, inserted at `session_insert_index` with the active-index fix-up), `src/gui/update/mod.rs:135-139` (**resize every discovered session to the computed dims immediately**, so tmux reports the right size on the first frame). `crates/grove-core/src/tmux.rs:296-311` (`list_grove_sessions` — intersects live tmux names with sidecars and **prunes** orphan sidecars), `crates/grove-core/src/session_meta.rs:42-84`. `crates/grove-core/src/session.rs:327-347` (`attach_existing`) and `:349-383` (`attach_tmux` — the exact `tmux -L <SOCKET> -u attach-session -t =<name>` command line, already reproduced verbatim at `crates/grove-gpui/src/entities/terminal_session.rs:616-627`).
  - **The `spawn_native` gap.** `crates/grove-core/src/session.rs:238-279` (`Session::spawn_native` — `identity.agent.invocation()`, then prefix args, then the caller's args, then the attention extra args, then cwd/TERM/LC_ALL/`GROVE_STATE_FILE`) vs `crates/grove-gpui/src/entities/terminal_session.rs:640-656` (a bare login shell). `src/app/spawn.rs:26-32,112-121` — where the caller's `args` come from: `agent.launch_args(self.skip_permissions_enabled(), self.chrome_enabled())`. `crates/grove-core/src/agent.rs:63-82,93-122`.
- **Interfaces Plans 03–08 already shipped — consume them, do not re-derive:**
  - `app::boot(cx)` — the single ordered startup sequence (`crates/grove-gpui/src/app.rs:50-135`). Telemetry appends to it at the marked step; **do not reorder the existing steps**, each one's comment says why it is where it is.
  - `settings::SettingsState::{new, update, flush_now, is_dirty}` (`src/settings.rs`) — the debounced store writer. `update` is the *only* way to mutate persisted settings; `flush_now` already exists and is waiting for this phase's callers.
  - `zoom::ZoomState` (`src/zoom.rs`) — the zoom value already lives here and already persists through `SettingsState::update`; this phase adds no second write path.
  - `entities::workspace_state::WorkspaceState::take_grid_order_to_persist` (`src/entities/workspace_state.rs:462-468`) — Plan 07's staging. Its drain is a quit-path concern and lands here.
  - `entities::session_registry::{SessionRegistry, SpawnTarget, SessionMeta, SessionId}` — spawning, home terminals, panel shells, insertion order.
  - `entities::terminal_session::{TerminalSession, Backend, spawn, spawn_script}` and `views::terminal_view::TerminalView` — one terminal renderer, no second one.
  - `modal::{Modal, ModalKind, ModalSlot}` and `views::modals::{ModalLayer, ModalEvent, ModalClick}` — the single-slot machine. `Modal::Updating` and `Modal::Changelog` already exist as variants with verdict-table rows (`src/modal.rs:218-224,321-322,825-833`) and a keyboard-matrix assertion that `Updating` refuses Escape mid-update (`src/keyboard_matrix.rs:502-519`) — **that assertion currently passes against a state that is never in flight; this phase must keep it passing against one that is.**
  - `views::appbar::AppbarCtx::upgrade_available`, `views::workspace::Workspace`, `entities::animation_clock`, `entities::activity_store`, `entities::toast::ToastState`.
- **Carried decisions (do not re-derive, do not re-open):**
  1. **grove-core is reused wherever it is public and UI-free — and for this phase that is everything.** `grove_core::upgrade` is the network layer, the semver comparison, the install-method classification and the apply strategies; grove-gpui writes **no** replacement for any of it and **no** second HTTP client. What grove-gpui owns is the orchestration `src/gui/update/upgrade.rs` owns in iced: when to check, what to do with the answer, and how to get a blocking call off the UI thread. Amendment protocol applies to anything that turns out not to be public.
  2. **The blocking/async boundary is gpui's background executor, not a raw thread.** iced runs `latest()`/`releases()` on the tokio executor via `Task::perform` and `apply()` on a hand-rolled `std::thread` with an `Arc<Mutex<UpgradeProgress>>` drained by the 60ms tick (`src/gui/update/upgrade.rs:92-111`, `src/gui/update/tick.rs:78-96`). In gpui all three are `cx.background_spawn(...)` awaited from `cx.spawn`, and the `Stage` callback posts through a channel the foreground task reads — **the tick-driven mutex drain does not get ported**, exactly as spec §4's "tick decomposition" says. The *observable* contract is unchanged and is what the tests assert: stages arrive in order, `Done`/failure lands exactly once, and the UI never blocks.
  3. **Telemetry is off unless a key was compiled in, and the default is `true`.** Three independent gates, all ported: `option_env!("GROVE_POSTHOG_KEY")` must be `Some` and non-empty; `GROVE_TELEMETRY=off|0|false` overrides everything; the stored `telemetry_enabled` preference gates the runtime atomic, which **starts `false`** so nothing transmits before the store is read (`src/telemetry.rs:12-32`). The stored preference's default is `unwrap_or(true)` (`src/app/mod.rs:331-333`). **grove-gpui currently reads it as `unwrap_or(false)`** (`src/views/modals/settings.rs:49,214`) — that is a parity bug this phase fixes, with a test, not a decision to preserve.
  4. **The panic message never leaves the machine; only the scrubbed location does.** Port `scrub_paths`/`scrub_token` with all four of their tests (`src/telemetry.rs:91-119,174-210`) and the hook's chaining of the previous hook (`src/main.rs:11-30`). The hook is installed in `main`, before `boot`, because a panic inside `boot` must still be reported.
  5. **The `spawn_native` gap is a PORT, not an accepted limitation — no user sign-off is required, because there is nothing to accept.** The master plan asked this plan to decide between porting `Agent::invocation()` plumbing to the native fallback and recording a platform limitation with the user's sign-off. Reading settles it: `Agent::invocation()` and `Agent::launch_args()` are **public, UI-free, already-tested grove-core functions** (`crates/grove-core/src/agent.rs:63-82,93-122`) that need no amendment, and the native fallback is not platform-constrained in any way — it is an unfinished port, explicitly marked "in spirit" in its own doc comment (`terminal_session.rs:637-639`). Task 6 completes it. **The gap is also strictly larger than Plan 06 recorded, and Task 6 owns the whole of it:** `SpawnTarget` (`src/entities/session_registry.rs:83-90`) carries no `args` field at all, so `Agent::launch_args(skip_permissions, chrome)` is dropped on the **tmux** path too (`terminal_session.rs:590-598` passes only the attention `extra_args` to `tmux::new_session`). Today, in grove-gpui, the Permissions and "Claude in Chrome" settings — and the Onboarding wizard's permissions choice — are **inert**: they persist and render, and no agent is ever launched with the flag. That is a silent behavioral divergence, not a missing feature, and it is the reason this task is not deferrable to Plan 10.
  6. **The quit path is `Window::on_window_should_close`, and it is the only close interception.** Findings §S4 pinned the API: `Window::on_window_should_close(&self, cx: &App, f: impl Fn(&mut Window, &mut App) -> bool + 'static)` (`gpui/src/window.rs`), return `false` to veto. That is the direct analogue of iced's `exit_on_close_request(false)` + `close_requests()` subscription, and it replaces both. Registration needs a `&mut Window`, which `main.rs` does not have at `open_window` time — register it inside the same `window.update(...)` block that already focuses the root and seeds the appearance (`crates/grove-gpui/src/main.rs:87-92`), or on `Workspace`'s first render as Plan 06 did for `observe_window_activation`; pick one, and record which and why in a comment.
  7. **Every process-terminating path flushes, and there are exactly three.** iced's list is authoritative (`src/gui/update/layout.rs:518-522`): the close request, the quit confirm, and the post-update restart. In gpui the flush is `SettingsState::flush_now(cx)` plus the `WorkspaceState::take_grid_order_to_persist` drain, and it must run on all three. Make that a **structural** property, not a discipline: one `fn shutdown(cx)` that both flushes and is the only thing any exit path calls. A test that enumerates the exit paths and asserts each one routes through it is worth more than the flush itself.
  8. **Reattach reconciliation is pure and TDD'd before any tmux runs.** `src/reattach.rs` takes the discovered list and the registry's current metas and returns a plan; nothing in it touches gpui or shells out. The two rules that make it non-trivial are both in the oracle: dedupe by tmux name so a re-scan after enabling tmux cannot double-insert (`src/app/mod.rs:352-366`), and honor `session_insert_index`'s ordering so a reattached session lands where the tree expects it. Startup and the tmux-toggle re-scan are the **same** function called twice, exactly as iced does.
- **Recorded ambiguities, resolved by reading the oracle:**
  1. **The launch check is 3 s after startup, and the 24h check deliberately does not fire at boot.** `maybe_check_updates_due` returns `false` when `last_update_check` is `None` — "launch check seeds the timestamp; don't double-fire at boot" (`src/gui/update/upgrade.rs:198-201`). Both rules are tests, not comments.
  2. **The refocus check exists because an idle unfocused window stops ticking.** `maybe_check_updates_due` is called from the tick *and* from the focus-regained path (its own doc comment, :193-196). In gpui the 60ms/1s tick is `AnimationClock`, so wire the periodic check to a **1s-granularity timer**, not to the animation tick — the check's own cadence is 24h and it has no business waking the clock.
  3. **`last_update_check` is written on every outcome, success or failure**, so a network-down machine backs off for 24h instead of retrying every tick (`:39-41`). Preserve that ordering: persist first, then branch on the result.
  4. **Manual and silent checks differ only in their error policy.** A manual check surfaces `UpgradeState::Error(e)`; a launch/periodic one logs and falls back to `Idle` with no badge and no modal (`:52-61`). The `manual` flag is threaded through the whole round trip for exactly this.
  5. **The changelog closes the Settings modal on the way in and reopens it on the way out** (`on_open_changelog` :130-131 sets `Modal::None` before showing the changelog; `on_close_changelog` :145-149 sets `Modal::Settings`). Plan 08 already modeled this as a real `Modal::Changelog` variant with the round trip as a state-machine test (`src/modal.rs:1028-1040`) — **do not re-model it**; this phase only fills in the fetch.
  6. **The Settings modal's Tools section has no gpui counterpart, and this plan does not build it.** iced's Settings renders a Tools list with per-agent availability, version and a refresh button (`src/gui/view/modals/settings.rs:400-430`), driven by `detect_tools_task` (`src/gui/update/upgrade.rs:158-191`, `Agent::available`/`Agent::version`). A grep of `crates/grove-gpui/src` for `ToolStatus`/`settings_tools`/`Tools` returns **nothing**. It is adjacent to this phase (same oracle file, same `Agent` API) but it is a **Modals** checklist row (Appendix A row 12, Plan 08 Task 7 Step 2), not a System row, and Plan 08's human sign-off is still pending. **Report it to the orchestrator in the completion note; build it only if the orchestrator authorizes it as an added Task.**
  7. **`UpgradeState::Updated` and `UpdateFailed` are terminal display states, not transitions.** After a successful apply the modal offers Restart; after a failure it shows the error string, which is `UpgradeError`'s `Display` and is deliberately the same text the iced modal shows (`crates/grove-core/src/upgrade.rs:11-14`). Do not re-word any of them.
  8. **Reattached sessions have no attention state and that is correct.** `attach_existing` passes `None` for attention (`crates/grove-core/src/session.rs:344`): the hook files are keyed `{grove-pid}-{session-id}` and a session that outlived the previous grove keeps appending to its old path, which the new process must not read. Classification therefore falls through to the native Claude poller (which works across restarts — `src/gui/update/tick.rs:181-195`) and then to screen-scrape `classify`. Plan 06 already ships both. **Do not invent a state-file rediscovery scheme**; `app::boot`'s `cleanup_stale_files` (`crates/grove-gpui/src/app.rs:58`) is GC only and stays GC only.
- No `git` commands until Task 7. Do not commit intermediate tasks. The orchestrator runs `./install.sh` and the commit.

---

### Task 1: Telemetry and the panic hook

**Files:**
- Create: `crates/grove-gpui/src/telemetry.rs`
- Modify: `crates/grove-gpui/src/{main.rs,app.rs,settings.rs,views/modals/settings.rs}`, `crates/grove-gpui/Cargo.toml`

**Interfaces:**
- Produces: `telemetry::{enabled, set_enabled, distinct_id, scrub_paths, track, track_blocking, start_heartbeat}`, the panic hook, and every event call site grove-gpui can reach.

- [ ] **Step 1: Port the module (tests first for the pure half)**

`src/telemetry.rs`, no gpui types. Port `src/telemetry.rs` verbatim in behavior: the `option_env!("GROVE_POSTHOG_KEY")` constant and the `us.i.posthog.com` endpoint (:4-5), the three gates in `enabled()` (:21-32), the `ENABLED` atomic that **starts false** and why (:12-15), `distinct_id()` with its `~/.config/grove/telemetry_id` persistence routed through `grove_core::storage::config_dir()` (:44-74), `generate_id()` (:76-83), the `send()` payload shape including `$process_person_profile: false`, `app_version`, `os` (:138-164), `track`/`track_blocking` and why both exist (:121-136), and `start_heartbeat` (:167-172).

**TDD the scrubber first.** `scrub_paths`/`scrub_token` (:91-119) come across with all four of their tests unedited (:174-210): home collapses to `~`, other absolute paths become `<path>`, a relative panic location survives untouched, and a home *prefix* mid-segment (`/home/tester2/x`) does **not** match. Red before green.

Add `ureq.workspace = true` to `crates/grove-gpui/Cargo.toml`. Confirm no new `Cargo.lock` entry appears (Task 7 checks this).

- [ ] **Step 2: The panic hook**

In `crates/grove-gpui/src/main.rs`, before `gpui_platform::application()`, install the hook ported from `src/main.rs:11-30`: take the previous hook, downcast the payload to `&str` then `String` then `"unknown panic"`, `tracing::error!` the message **locally**, transmit only `scrub_paths(file:line:col)` via `track_blocking`, then chain the previous hook. The ordering is load-bearing — install it before `app::boot` so a panic inside boot is still reported.

- [ ] **Step 3: Init, at the marked step in `boot`**

Replace the `crates/grove-gpui/src/app.rs:111` marker with the three calls iced makes in `Grove::new` (`src/gui/update/mod.rs:102-118`), in order: `set_enabled(store.telemetry_enabled.unwrap_or(true))`, then `track("app_launched", [theme, project_count, tmux_enabled])`, then `start_heartbeat()`. `theme` falls back to the literal `"default"` when unset. Place them **after** the store loads and **before** the globals are installed; leave every existing numbered step where it is.

- [ ] **Step 4: Fix the `telemetry_enabled` default (carried decision 3)**

`crates/grove-gpui/src/views/modals/settings.rs:49,214` read `store.telemetry_enabled.unwrap_or(false)`; the oracle is `unwrap_or(true)` (`src/app/mod.rs:331-333`). Add `SettingsState::telemetry_enabled()` as the single accessor, route both call sites and `boot` through it, and pin the default with a test. Toggling the row must also call `telemetry::set_enabled` in the same update, mirroring `src/app/mod.rs:339-344`.

- [ ] **Step 5: Wire every reachable event call site**

Port each one to its grove-gpui equivalent, keeping the event name and property names **byte-identical** — they are a wire format:
`app_launched` (Step 3); `heartbeat` (Step 3); `panic` (Step 2); `session_created` (`src/gui/update/sessions.rs:463-472`) and `error`/`spawn_failed` (:480) → `Sidebar::spawn_session`, beside the toast producer Plan 08 landed at `views/sidebar.rs:373-380`; `session_ended` (`sessions.rs:261-269`) → wherever a session is removed from the registry; `worktree_created` and `error`/`worktree_failed` (`src/app/spawn.rs:221-229`); `launcher_opened` (`src/gui/update/palette.rs:15`); `settings_opened` (`src/gui/update/mod.rs:552`); `zoom_changed` on all three zoom actions (:537,542,547); `tile_moved` (:1029); `update_declined` and `update_applied` (Task 4). Any call site whose grove-gpui counterpart genuinely does not exist yet is **listed in the completion note**, not silently skipped.

- [ ] **Step 6: Verify**

```bash
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 test -p grove-gpui telemetry 2>&1 | tail -40
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 clippy -p grove-gpui --all-targets --no-deps -- -D warnings 2>&1 | tail -20
# No key is compiled in here, so nothing may leave the machine during tests:
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 test -p grove-gpui 2>&1 | tail -20
```

---

### Task 2: Quit paths and the persistence flush

**Files:**
- Modify: `crates/grove-gpui/src/{main.rs,settings.rs}`, `crates/grove-gpui/src/views/workspace.rs`, `crates/grove-gpui/src/views/modals/confirm.rs`

**Interfaces:**
- Produces: `on_window_should_close` interception, the quit confirm's real gate, and one `shutdown` that every exit path routes through.

- [ ] **Step 1: `shutdown` — one flush, three callers (carried decision 7, tests first)**

A single function that (a) drains `WorkspaceState::take_grid_order_to_persist` into `Store::grid_order` through the stable session key, mirroring `persist_grid_order` (`src/gui/update/layout.rs:486-489`), and (b) calls `SettingsState::flush_now(cx)` — the caller `crates/grove-gpui/src/settings.rs:10,68-83` has been waiting for since Plan 03. It must be idempotent: calling it twice writes once, because `flush_now` already no-ops when `!is_dirty` and the drain already returns `None` the second time. Test both properties.

- [ ] **Step 2: Close-request interception (carried decision 6, findings §S4)**

Replace the `crates/grove-gpui/src/main.rs:72` marker. `Window::on_window_should_close(&self, cx, f) -> bool`: compute the **running native** session count from the registry — tmux-backed sessions survive grove and must never block a quit (`src/gui/update/modals.rs:339-341`, `src/app/mod.rs:275`). Zero → `shutdown(cx)` and return `true`. Otherwise open `Modal::Confirm { kind: ConfirmKind::Quit, .. }` with the oracle's exact copy — title `"Quit Grove?"`, prompt `"{n} running {session|sessions} will end. quit anyway?"`, `destructive: true` (`modals.rs:346-360`) — and return `false`.

The quit confirm **clobbers** whatever modal is open and cancelling does not restore it. That is a preserved gap, it is already a passing test in `src/modal.rs`, and this step must not "improve" it.

- [ ] **Step 3: The quit confirm's confirm arm**

`views/modals/confirm.rs:145` currently notes that the flush is Plan 09's. `ModalEvent::Quit` reaches `Workspace::on_modal_event` (`views/workspace.rs:891`) and calls `cx.quit()`; put `shutdown(cx)` immediately before it, and delete both markers. Oracle: `submit_modal_confirm` → `iced::exit()` (`src/gui/update/modals.rs:558-576`).

- [ ] **Step 4: Prove the debounce actually survives a quit**

The zoom debounce is 250ms (`ZOOM_SAVE_QUIET_TICKS = 4` × 60ms, `src/gui/update/mod.rs:52-56`; gpui's is a `Timer` per Plan 03). Add a test that mutates a setting, does **not** wait for the debounce, runs `shutdown`, and asserts the value is on disk. That is the entire point of `flush_ui_zoom_save` and it has never been asserted in either UI.

- [ ] **Step 5: Verify**

```bash
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 test -p grove-gpui 2>&1 | tail -40
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 clippy -p grove-gpui --all-targets --no-deps -- -D warnings 2>&1 | tail -20
grep -rn "Plan 09" crates/grove-gpui/src   # main.rs:72, settings.rs, workspace.rs:891, confirm.rs:145 gone
```

---

### Task 3: The upgrade state machine (pure, TDD)

**Files:**
- Create: `crates/grove-gpui/src/entities/upgrade_state.rs`
- Modify: `crates/grove-gpui/src/entities/mod.rs`

**Interfaces:**
- Produces: `UpgradeState`, `ChangelogState`, and every upgrade decision as a tested pure function — before a single network call exists.

- [ ] **Step 1: The state types** (`src/gui/state.rs:434-458`)

`UpgradeState::{Idle, Checking, UpToDate, Available(Release), Updating(Stage), Updated, UpdateFailed(String), Error(String)}` and `ChangelogState::{Idle, Loading, Loaded(Vec<ReleaseNote>), Error(String)}`, over `grove_core::upgrade`'s own types. No gpui, no `Entity`, no I/O.

- [ ] **Step 2: The decisions, each a named test**

- `check_due(last_update_check: Option<u64>, now: u64, state: &UpgradeState) -> bool` — 24h elapsed **and** state is `Idle | UpToDate`; `None` is **not** due (recorded ambiguity 1; `src/gui/update/upgrade.rs:197-207`).
- `begin_check(state) -> bool` — the in-flight guard: a second check while `Checking` is refused (`:26-32`).
- `apply_check_result(result, manual, current, skipped) -> UpgradeState` — `update_available` decides Available vs UpToDate (`:42-51`); on error, `manual` → `Error(e)`, otherwise `Idle` (recorded ambiguity 4, `:52-61`). Cover the skipped-tag and newer-than-skipped cases — `grove_core::upgrade::update_available` already tests those, so assert the *policy*, not semver.
- `skip_version(state) -> (Option<String>, UpgradeState)` — records the tag to persist and lands on `UpToDate` (`:65-75`).
- `apply_progress(state, stage) -> UpgradeState` and `apply_finished(state, Result<(),String>) -> UpgradeState` — `Updating(stage)` in order, then `Updated` / `UpdateFailed(e)` exactly once (`src/gui/update/tick.rs:87-95`). Assert a late `Stage` arriving **after** a finish cannot resurrect `Updating`; the iced drain reads both fields in one lock and is ordering-safe by construction, and the channel port must be too.
- `upgrade_available(state) -> bool` — the cog dot condition, `matches!(state, Available(_))` and nothing else (`src/gui/view/appbar.rs:29`).
- `escape_closes(state) -> bool` — false while `Updating`, true otherwise. This is the predicate `src/keyboard_matrix.rs:502-519` already asserts against; hook the existing assertion up to the real state and confirm it still passes.

- [ ] **Step 3: Verify**

```bash
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 test -p grove-gpui upgrade 2>&1 | tail -40
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 clippy -p grove-gpui --all-targets --no-deps -- -D warnings 2>&1 | tail -20
```

---

### Task 4: The live upgrade flow — checks, stages, changelog, apply/restart, the cog dot

**Files:**
- Create: `crates/grove-gpui/src/entities/upgrade.rs`
- Modify: `crates/grove-gpui/src/views/{workspace.rs,modals/settings.rs}`, `crates/grove-gpui/src/main.rs`

**Interfaces:**
- Produces: the `Upgrade` entity, the three check triggers, the Updating modal's real stages, the changelog fetch, apply + restart, and the cog's green dot.

- [ ] **Step 1: The entity and its executor boundary (carried decision 2)**

`Upgrade` holds Task 3's `UpgradeState`, the `ChangelogState`, and `InstallMethod` from `grove_core::upgrade::detect()` (resolved once, at construction — `src/gui/update/mod.rs:198`). Three background operations, all `cx.background_spawn` awaited from `cx.spawn`, all going through grove-core:
- `latest()` → `apply_check_result` (`crates/grove-core/src/upgrade.rs:253-277`);
- `releases(10)` → `ChangelogState` (`:639-654`, limit from `src/gui/update/upgrade.rs:227`);
- `apply(method, &release, &progress)` (`:281-297`) — the `progress` callback is `Fn(Stage) + Send + Sync` and posts each stage down a channel the foreground task drains into `apply_progress`, with the final `Result` into `apply_finished`. **No `Arc<Mutex<..>>` drained by a tick.**

Persist `last_update_check` **before** branching on the result (recorded ambiguity 3, `src/gui/update/upgrade.rs:39-41`), through `SettingsState::update`.

- [ ] **Step 2: The three triggers**

- **Launch:** ~3 s after startup, once (`src/gui/mod.rs:56-63`). A `Timer` in a `cx.spawn` from `Workspace`'s construction; the 3 s exists so the first frame is up before the network round-trip, so do not shorten it.
- **Periodic:** a 1 s-granularity timer asking `check_due` (recorded ambiguity 2 — **not** the AnimationClock).
- **Refocus:** the same `check_due` question on window activation. Plan 06 already registered `observe_window_activation` for the attention acknowledge (master row 06); add the check there rather than registering a second observer.

All three route through `begin_check`, so a duplicate is impossible by construction.

- [ ] **Step 3: The modals stop being shells** (`src/gui/view/modals/upgrade.rs:16-97,98-182`)

`views/modals/settings.rs:668-706` renders the upgrade shell and `:707+` the changelog; both currently say Plan 09 fills them (`:224,668,685`, `views/modals/mod.rs:343`). Fill them: the available-release view with its version, notes (`clean_markdown`, `crates/grove-core/src/upgrade.rs:659-700`), Update / Skip / Copy-URL actions; the `Updating` view showing the live `Stage`; the terminal `Updated` (offering Restart) and `UpdateFailed(e)` states. `Modal::Changelog` renders `ChangelogState`'s three states and returns to `Settings` on dismiss — the round trip is **already** a passing state-machine test (`src/modal.rs:1028-1040`) and this step only supplies the data. `SettingRow::CheckUpdates` (`views/modals/settings.rs:223`) runs a **manual** check instead of blindly opening `Modal::Updating`.

Escape must now be genuinely refused mid-update: `views/modals/mod.rs:343`'s "nothing is ever in flight" comment is deleted and `escape_closes` becomes the real answer.

- [ ] **Step 4: Skip, copy URL, restart**

`skip_version` persists `Store::skipped_version` and fires `update_declined` (`src/gui/update/upgrade.rs:65-75`). Copy-URL goes through the existing clipboard path and raises the `"release url copied"` toast (:77-82). **Restart** ports `on_restart_app` (:115-125) in its exact order: spawn `current_exe()` (logging a failed relaunch, because the process is about to exit either way), then **`shutdown(cx)` from Task 2**, then exit. A successful apply fires `update_applied` with the tag (:102-107).

- [ ] **Step 5: The cog dot**

`views/workspace.rs:1380` hardcodes `upgrade_available: false`; feed it `upgrade_available(state)`. Delete the stub comments at `views/appbar.rs:84,217`. The dot's own rendering already exists and is not touched.

- [ ] **Step 6: Verify**

```bash
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 test -p grove-gpui 2>&1 | tail -40
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 clippy -p grove-gpui --all-targets --no-deps -- -D warnings 2>&1 | tail -20
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 test -p grove-gpui keyboard_matrix 2>&1 | tail -20
```

---

### Task 5: tmux sidecar discovery and reattach

**Files:**
- Create: `crates/grove-gpui/src/reattach.rs`
- Modify: `crates/grove-gpui/src/entities/{terminal_session.rs,session_registry.rs}`, `crates/grove-gpui/src/views/workspace.rs`, `crates/grove-gpui/src/main.rs`

**Interfaces:**
- Produces: sessions that survive a grove restart — the single largest user-visible thing grove-gpui cannot currently do.

- [ ] **Step 1: The pure reconciliation (carried decision 8, tests first)**

`src/reattach.rs`, no gpui, no shelling out. Input: the discovered list (`Vec<DiscoveredSession>` from `grove_core::tmux::list_grove_sessions`, `crates/grove-core/src/tmux.rs:296-311`) plus the registry's current metas. Output: which to attach, in insertion order. Rules, each a test:
- a discovered session whose tmux name is already in the registry is **skipped** (`src/app/mod.rs:352-358`) — this is what makes startup and the tmux-toggle re-scan the same function;
- ordering follows the registry's existing insertion rule so a reattached session lands where the tree expects it (`:360-365`);
- an empty discovery list is a no-op;
- the function is total: it never panics on a meta with an unknown project or a worktree path no longer on disk (the sidecar outlives the checkout).

- [ ] **Step 2: `TerminalSession::attach_existing`**

The tmux attach command line already exists verbatim in `spawn_tmux` (`crates/grove-gpui/src/entities/terminal_session.rs:616-627`: `tmux -L <SOCKET> -u attach-session -t =<name>`, `TERM`/`LC_ALL`). Factor it out and add a constructor that **skips** `tmux::new_session` and `session_meta::write` — the session and its sidecar already exist — and does the three things `attach_tmux` does (`crates/grove-core/src/session.rs:349-383`): `configure_embedded_session`, spawn the attach PTY, capture `tmux::pane_pid`. Attention is **`None`** (recorded ambiguity 8) and the reason goes in a comment.

- [ ] **Step 3: Startup and the tmux-toggle re-scan**

Call it at startup **only when `tmux::available()`** (`src/app/mod.rs:219-224`), and again when the Settings tmux row switches the backend on (`src/app/mod.rs:288-292`). Reattached sessions keep their tmux backend even when the saved preference now says native — the oracle says so explicitly (`:217-219`).

Then port the detail that makes the first frame right: **resize every reattached session to the computed PTY dims immediately** (`src/gui/update/mod.rs:135-139`), so tmux does not report a stale geometry.

- [ ] **Step 4: Failures are silent**

`discover_sessions` drops sessions that fail to attach with no toast and no modal (`src/app/util.rs:12-13`). Log at `warn`, continue with the rest. A tmux server that died between `list-sessions` and the attach must not stop grove from starting.

- [ ] **Step 5: Verify**

```bash
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 test -p grove-gpui reattach 2>&1 | tail -40
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 test -p grove-gpui 2>&1 | tail -20
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 clippy -p grove-gpui --all-targets --no-deps -- -D warnings 2>&1 | tail -20
```

---

### Task 6: The `spawn_native` agent-invocation gap (carried decision 5)

**Files:**
- Modify: `crates/grove-gpui/src/entities/{terminal_session.rs,session_registry.rs}`, `crates/grove-gpui/src/views/sidebar.rs`

**Interfaces:**
- Produces: a native fallback that actually runs the agent, and launch flags that stop being inert on **both** backends.

- [ ] **Step 1: `SpawnTarget` carries the agent's args**

`SpawnTarget` (`src/entities/session_registry.rs:83-90`) has `cwd`/`agent`/`project`/`label` and no args. Add the launch args, built where the caller builds them in iced — `agent.launch_args(skip_permissions_enabled, chrome_enabled)` (`src/app/spawn.rs:26-32`, `crates/grove-core/src/agent.rs:63-82`), read from the store through `SettingsState`. `SpawnTarget::home` (`:92-103`) keeps an empty arg list: `Agent::Terminal` has no flags.

- [ ] **Step 2: The native fallback invokes the agent**

`spawn_native` (`crates/grove-gpui/src/entities/terminal_session.rs:640-656`) currently spawns a bare login shell. Port `Session::spawn_native` (`crates/grove-core/src/session.rs:238-279`) exactly: `agent.invocation()` gives `(program, prefix_args)`; then prefix args, then the target's launch args, then the attention `extra_args`, then `cwd`, `TERM=xterm-256color`, `LC_ALL=en_US.UTF-8`, and `GROVE_STATE_FILE` when there is one. `Agent::Terminal` resolves through the same call and still yields the login shell, so home terminals and panel shells are unaffected — assert that.

Its doc comment's "in spirit" hedge (:637-639) is replaced by a statement of what it now does.

- [ ] **Step 3: The tmux path gets the launch args too**

`spawn_tmux` passes only `extra_args` to `tmux::new_session` (`terminal_session.rs:590-598`); the oracle chains the caller's args **first** and the attention args after (`crates/grove-core/src/session.rs:190`). Fix the chain order. This is the half of the gap Plan 06 did not record, and it is why the Permissions / Claude-in-Chrome settings and the Onboarding permissions choice currently do nothing in grove-gpui.

- [ ] **Step 4: `root_pid` for native sessions**

Plan 06 recorded `TerminalSession::root_pid()` returning `None` for `Backend::Native` as harmless "since native never runs an agent". After Step 2 that premise is false: a native Claude session is now real, and the Claude poller keys on the root pid (`src/gui/update/tick.rs:191-195`). Determine whether `PtyHandle` can expose a child pid **without editing `crates/grove-terminal`** (it is read-only under Constraint 3). If it cannot, **STOP and report** — the orchestrator authorizes a grove-terminal amendment or accepts the fallback, which is the poller's cwd-uniqueness heuristic (`crates/grove-core/src/claude_agents.rs`). Do not edit grove-terminal on your own judgement, and do not leave this undecided.

- [ ] **Step 5: Verify**

```bash
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 test -p grove-gpui 2>&1 | tail -40
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 clippy -p grove-gpui --all-targets --no-deps -- -D warnings 2>&1 | tail -20
git status --short crates/grove-terminal   # expect EMPTY
```

---

### Task 7: Verification and the System checklist

**Files:**
- Modify: `docs/superpowers/plans/2026-07-31-gpui-rewrite-00-master.md` (row 09 → done)

**Interfaces:**
- Produces: the phase's evidence and the exit gate.

- [ ] **Step 1: Automated**

```bash
cd /home/gitfudge/dev/gitfudge0/grove
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 build -p grove-gpui 2>&1 | tail -5
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 test -p grove-gpui 2>&1 | tail -40
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 test -p grove-gpui keyboard_matrix 2>&1 | tail -20
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 clippy -p grove-gpui --all-targets --no-deps -- -D warnings 2>&1 | tail -20
GROVE_CONFIG_DIR=$(mktemp -d) GROVE_GPUI_SELFTEST=1 PATH="$HOME/.cargo/bin:$PATH" \
  cargo +1.95.0 run -p grove-gpui 2>&1 | tail -5
# exactly one gpui, and no new HTTP client:
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 tree -p grove-gpui -i gpui 2>&1 | head -20
grep -c '^name = "gpui"' Cargo.lock
grep -cE '^name = "(reqwest|hyper|isahc|curl)"' Cargo.lock
# every Plan 09 marker is gone:
grep -rn "Plan 09" crates/grove-gpui/src
# the rest of the workspace, DEFAULT toolchain — must be untouched
rustc --version
cargo build 2>&1 | tail -5
cargo test 2>&1 | tail -10
git status --short src crates/grove-core crates/grove-terminal   # expect EMPTY
rustfmt --edition 2021 --check crates/grove-gpui/src/*.rs crates/grove-gpui/src/*/*.rs crates/grove-gpui/src/views/modals/*.rs
```

Expected: everything green; `grep -c '^name = "gpui"'` is `1`; the HTTP-client grep is `0`; the Plan 09 grep returns **nothing**; the Plan 03 metric selftest still prints its `cell_w=7.5… OK` line; `git status` reports no changes under `src/`, `crates/grove-core/`, `crates/grove-terminal/`.

- [ ] **Step 2: MANUAL — the spec Appendix A *System* rows (human, real desktop)**

```bash
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 run --release -p grove-gpui
# and, side by side, the installed iced build:
~/.local/bin/grove
```

Report each row pass/fail. **Do not claim any of these yourself.** Rows 1–10 are spec Appendix A's *System* clause, verbatim and in order.

1. **31 + custom themes, 11 tokens, follow-system with the startup seed order** — every builtin and every `themes.json` custom theme resolves; the 11 tokens and their contrast partners render identically to iced; flipping the OS appearance flips the app; a fresh launch in follow-system mode paints the **right** mode on the first frame, not a flash of the wrong one. *(Shipped in Plan 03; verified here as a System row.)*
2. **Zoom 0.6–2.0 whole-app, driving logical PTY dims** — every step re-lays the chrome and the terminal together, and the PTY's reported `(rows, cols)` matches iced at the same window size and zoom. *(Plan 03/07.)*
3. **Sidebar width / zoom / grid order / settings persisted** — set all four, quit **normally**, relaunch: all four come back. Then set the zoom and quit **within 250ms** (the debounce window) — it must still come back. Then rearrange the grid and quit from grid view — the order comes back.
4. **tmux sidecar discovery/reattach** — with live agent sessions, quit grove and relaunch: every tmux-backed session reappears in the tree with its project/worktree/label/agent intact and its scrollback live; native sessions do **not** reappear; a session killed outside grove leaves no ghost row and no orphan sidecar; the reattached terminal is sized correctly on the first frame, not after a resize. Then turn tmux **off** and back **on** in Settings and confirm the re-scan neither duplicates nor drops a session.
5. **Attention stale-file GC at startup** — leftover state files from a previous run are cleared before any session spawns, and a reused session id cannot read a stale file. *(Plan 03/06; `crates/grove-gpui/src/app.rs:58`.)*
6. **Login-PATH recovery** — launched from a desktop launcher (not a shell), agents still resolve on `$PATH`. *(Plan 03; `app.rs:54`.)*
7. **Panic hook + telemetry** — with no key compiled in, **nothing** is transmitted (confirm with a network monitor or by trusting the three gates and reading the code, and say which); the panic hook logs the message locally and would transmit only a scrubbed location; the Settings **Telemetry** row defaults to on, persists, and flipping it takes effect immediately.
8. **arboard + OSC52 clipboard** — copy a terminal selection and paste into another app; paste in; on Wayland the `wl-paste` file-URI fallback still works. *(Plan 04; the Wayland round-trip caveat is findings §S4 Deviation 4 and its full sweep is Plan 10.)*
9. **Adaptive 60ms/1s tick and its gating** — an unfocused, quiet window drops to the slow cadence while a background agent still streams and classifies at full rate. *(Plan 03/06; the numeric **idle-power measurement** is Plan 10 — eyeball only here.)*
10. **3s-delayed + 24h/refocus upgrade checks, changelog, apply/restart** — the launch check fires ~3 s after startup and not before; a newer release lights the **green dot on the cog** and offers Update / Skip / Copy URL; Skip suppresses that tag and a *newer* tag surfaces again; a manual check from Settings surfaces its error inline while a silent one stays quiet; the changelog fetches the 10 most recent releases, renders them, and **returns to Settings** on dismiss; starting an update shows live Downloading → Building → Installing stages with the window still responsive, **Escape is refused while it runs**, and Restart relaunches into the new version with the sidebar width and zoom intact.

Rows explicitly **deferred** and not checked here (record as deferred, not failed): the scripted screenshot sweep and the numeric idle-power measurement → **Plan 10**; the macOS dock badge/bounce and mac-only ⌘ chords → **Plan 10 on a macOS host**; IME composition and the full Wayland clipboard round-trip → **Plan 10's manual sweep**; deleting vt100 and the iced app → **Plan 10**. The Settings **Tools** section (recorded ambiguity 6) is **not** a System row — report its absence to the orchestrator as a Plan 08 Modals-row finding.

- [ ] **Step 3: `./install.sh`** — the orchestrator runs this.

```bash
./install.sh 2>&1 | tail -20
```

Expected: the release build + install of the **iced** `grove` binary still succeeds, untouched by this phase.

- [ ] **Step 4: Update the master plan and commit** — the orchestrator runs this.

Mark row 09 `done` in `docs/superpowers/plans/2026-07-31-gpui-rewrite-00-master.md` with a one-line note recording: whether any grove-core **or grove-terminal** amendment had to be authorized (expected: none, unless Task 6 Step 4 forced one); the final test count and that clippy is clean on 1.95; that no new HTTP dependency entered `Cargo.lock` and every network call goes through `grove_core::upgrade`; **the `spawn_native` scope outcome** — that the gap was ported rather than accepted, that it was larger than Plan 06 recorded (launch args were inert on the tmux path too), and what Task 6 Step 4 decided about `root_pid` for native sessions; whether the `telemetry_enabled` default fix changed observable behavior; which telemetry call sites had no grove-gpui counterpart; that every `Plan 09` marker is gone from the source; and any Appendix A System row that came back FAIL or MANUAL-deferred.

```bash
git add crates/grove-gpui Cargo.toml Cargo.lock docs/superpowers/plans/2026-07-31-gpui-rewrite-00-master.md
git commit -m "feat(gpui): upgrade flow, telemetry, quit paths, persistence, tmux reattach"
```

**Exit gate met when:** the Appendix A *System* rows above are signed off by a human as pass or explicitly-deferred, `grep -rn "Plan 09" crates/grove-gpui/src` returns nothing, the keyboard matrix is still green with `Updating`'s Escape now refused against a *real* in-flight state, `Cargo.lock` gained no HTTP client and still holds exactly one `gpui` at ZED_REV, grove-gpui builds/tests/clippy clean on 1.95, the iced app and both existing crates are provably untouched and still build on the default toolchain, and `./install.sh` is green.
