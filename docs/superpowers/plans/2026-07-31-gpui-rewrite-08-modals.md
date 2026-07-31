# gpui Rewrite Plan 08: Modals, simple → text-heavy

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. This is **production** code: the workspace clippy denies apply (`unwrap_used`/`expect_used`), superpowers:test-driven-development governs every pure helper (tests before implementation, red before green), and superpowers:verification-before-completion governs every "done" claim — read raw command output, never a summary line. Also load the `gpui-development` skill before writing any gpui code; training-data gpui is stale and this rev is pinned.

**Goal:** Grove-gpui has four screens and no modals. `Modal` does not exist as a concept in the crate: the appbar's `+`, the cog, both statusbar chips, the session bar's `run script`, every sidebar row action (`AddWorktree`/`DeleteWorktree`/`RemoveProject`/`ProjectScripts`/`RunScript`/`AddProject`) and five registry actions (`NewSession`, `NewSessionInWorktree`, `SwitchSession`, `Settings`, `ShortcutOverlay`) all dispatch to logged Plan 08 stubs (`views/workspace.rs:363-367,431,1228-1232`, `views/sidebar.rs:228-232,617`). This phase builds the **single-slot modal layer** and all nineteen modals behind it, in risk order: keyboard-only dialogs first, then the wizards, then the text-heavy palettes and editors. It is also the phase that finally takes the `gpui-component` dependency, which means it opens with the **durable-pin decision Plan 03 deferred here** — and it is the phase where the Plan 04 `should_forward` deviation (deviation 7) gets paid off, because "which keys a focused text field is allowed to steal" is exactly what a modal layer decides.

Exit gate (master plan row 08): **all modal checklist rows green; keyboard matrix green** (spec Appendix A → the *Modals* paragraph, enumerated verbatim in Task 7 Step 2, plus spec §8.2's keyboard matrix, enumerated as an automated table test in Task 7 Step 1); `./install.sh` green; one commit.

**Out of scope — do not build it here.** The upgrade flow's live stages (`UpgradeState::Updating` polling, changelog fetch, apply/restart) — the `Updating` and changelog modals ship here as **shells** driven by whatever `UpgradeState` already reports, and Plan 09 fills them; telemetry, quit-path persistence beyond what the quit confirm needs, tmux sidecar discovery/reattach (Plan 09). The scripted screenshot sweep across every modal × 3 zooms × 4 themes (Plan 10). The macOS dock badge/bounce sign-off (Plan 10 on a macOS host). Deleting the iced app (Plan 10). Do not touch the 480ms activity task, the grid math, or the terminal element beyond the one live-preview hook named in Task 6.

**Architecture (new/changed files only):**

```
vendor/gpui-component/          NEW (Task 1). Durable pin: a plain copy of
                                longbridge/gpui-component @ 88f102d, crates
                                `ui` + `macros` + `assets` only, with README.md
                                recording source/rev/license (Apache-2.0) and
                                LICENSE-APACHE carried over. Workspace-excluded,
                                consumed by path.
crates/grove-gpui/
  src/modal.rs                  NEW. The pure modal state machine: `Modal` (the
                                single slot), `ModalKind`, `open`/`replace`/
                                `cancel`/`close`, the per-modal Escape verdicts,
                                the key-context strings, and the quit-confirm
                                clobber rule. No gpui types; all TDD.
  src/views/modals/
    mod.rs                      NEW. `ModalLayer` — one entity, one slot, the
                                scrim, focus-on-mount, Escape routing.
    shell.rs                    NEW. The shared chrome ported from
                                `src/gui/widgets/modal.rs`: panel, header,
                                footer hints, keycaps, action buttons, checkbox.
    input.rs                    NEW. `ModalInput` — the gpui-component `Input`
                                wrapper that owns BOTH S2 workarounds.
    confirm.rs                  NEW. Confirm (incl. Quit), Message, Input,
                                TmuxChoice, AgentPicker.
    project.rs                  NEW. RemoveProject + teardown progress,
                                ArchiveProject, ArchivedProjects, Teardown.
    add_project.rs              NEW. The two-step wizard + dir autocomplete.
    onboarding.rs               NEW. The full-viewport first-run wizard.
    launcher.rs                 NEW. The recents-first palette + drill-ins.
    theme_picker.rs             NEW. Dark/light tabs, follow-system, scopes.
    theme_manager.rs            NEW. List sub-view + the multiline editor.
    settings.rs                 NEW. Settings (immediate persist), incl. the
                                archived-projects row and the tmux setting.
    shortcuts.rs                NEW. Registry-generated shortcut overlay.
    scripts_editor.rs           NEW. The three-multiline-editor view.
    upgrade.rs                  NEW. Updating + changelog shells (Plan 09 fills).
  src/launcher.rs               NEW. The palette's PURE half: row building,
                                fuzzy ranking, identity resolution, scroll
                                offsets, recents ordering.
  src/views/workspace.rs        MODIFIED: hosts `ModalLayer`, the five stub
                                actions become real, chrome dispatch goes live.
  src/views/sidebar.rs          MODIFIED: row actions open modals; the
                                agent-session toast producer lands in
                                `Sidebar::spawn_session`.
  src/views/statusbar.rs        MODIFIED: the palette and shortcuts chips.
  src/views/session_header.rs   MODIFIED: `run script` stops being a stub.
  src/terminal_element.rs       MODIFIED: the one live-theme-preview hook
                                (`terminal_element.rs:156`).
  src/keymap.rs                 MODIFIED: modal key contexts.
```

**Tech stack additions:** `gpui-component` (Apache-2.0), vendored — see Task 1. Nothing else. gpui/alacritty pins unchanged.

## Global Constraints

- Branch: `gpui-rewrite`. Toolchain regime is **identical to Plans 03–07** and is not re-litigated:
  - grove-gpui builds/tests/clippy only via `PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 -p grove-gpui`.
  - Bare `cargo build` / `cargo test` (default-members, rustc 1.94.1) must keep working untouched for `grove`, `grove-core`, `grove-terminal`. Never run `--workspace`.
  - clippy for grove-gpui runs **`--no-deps`**: `cargo +1.95.0 clippy -p grove-gpui --all-targets --no-deps -- -D warnings`. The vendored crates are *deps*; `--no-deps` is what keeps their lints out of our build, and they must never be workspace members (Task 1).
  - `rustfmt --edition 2021` on **touched files only**, and **never** on anything under `vendor/`.
- **Pins are law.** ZED_REV `1a246efd7e1b83ab568ec5e3e6c1a43a42e1abba`; alacritty fork `4c129667ce56611becdc82de6e28218c80e2e88f`; GPUI_COMPONENT_REV `88f102d13654fe25aa2fede076274b6b751a3704`. The vendored copy is frozen at that rev and is not "updated" during this phase for any reason.
- **Constraint 3 — grove-core and the iced app are read-only.** No edits under `src/`, `crates/grove-core/`, or `crates/grove-terminal/`. Amendment protocol unchanged: if a genuinely **UI-free** helper must be exposed from grove-core, **STOP and report**; the orchestrator authorizes it. Do not edit grove-core on your own judgement.
  - **Expected outcome this phase: no amendment is needed.** Everything the modals persist already exists on `Store`/`storage` (projects, archived projects, scripts, settings, themes, recents) and the destructive paths already exist as grove-core operations (`git worktree` add/remove, teardown, archive). The wizards' *presentation* logic (`src/gui/add_project.rs`, `src/gui/onboarding.rs`, `src/gui/session_launcher/`) lives in the **iced crate**, which is read-only-as-oracle, not as a dependency — reimplement it in grove-gpui with its tests, exactly as Plans 05/07 did.
  - **Foreseen candidate spots** (report, do not act): (1) `grove_core::session::Session` — still forbidden, it owns the vt100 parser; the Teardown modal's embedded PTY is a `TerminalSession` from the registry, not a `Session`. (2) directory-listing/autocomplete helpers behind `add_project` — if they turn out to live in grove-core rather than in `src/gui/add_project.rs`, read them, do not move them.
- **Behavior questions are answered by reading the iced code, never by guessing.** Canonical oracles for this phase, cited per task:
  - `src/app/modal.rs:5-150` — **the modal inventory itself**, one variant per modal with the doc comments that explain which state lives on `Modal` (cloneable) versus on `Grove` (not cloneable: `add_project`, `launcher`, `scripts_editor`, `theme_manager_editor`, `teardown`). `:152-175` `Teardown`/`TeardownStage`, `:177-186` `ConfirmKind` (incl. `Quit`).
  - `src/gui/update/modals.rs` — the lifecycle authority: `handle_modal_key` (**:94-336 — this is the keyboard matrix's per-modal oracle, read it end to end before Task 2**), `handle_remove_project_key` (:69-93), `on_close_requested` (:338-366), `on_key_press` (:367-384), `on_modal_confirm` (:474-489), `choose_tmux` (:535-540), `submit_modal_input` (:541-557), `confirm_modal_response` (:558-576), `submit_modal_confirm` (:577-604), `focus_add_project_field` (:625-644), **`set_modal` (:645-651)**, **`open_child` (:669-676)**, **`cancel_modal` (:677-702)**, `archive_gate_sessions` (:703-720), `open_archive_gate`/`refresh_archive_gate` (:721-745), `on_archive` (:746-796), `kick_off_remove_project` (:797-856), `advance_remove_project` (:857-906).
  - `src/gui/view/modals/mod.rs:30-151` — `modal_layer`: the single `match` that renders the slot, the shared `SCRIM()` centering container, and the two documented exceptions (Onboarding short-circuits in `view()` and never reaches the layer; SessionLauncher top-drops instead of centering).
  - `src/gui/widgets/modal.rs` — the shared chrome: `modal_panel` (:13), `keycap` (:44), `keycap_text` (:60), `section_header` (:77), `footer_hint` (:95), `footer_container` (:109), `modal_footer_hints` (:135), `modal_footer_row` (:148), `modal_header` (:156), `modal_header_row` (:162), `palette_input_style` (:173), `ModalBtn` (:193), `modal_action` (:202), `modal_action_sized` (:212), `modal_checkbox` (:256).
  - `src/gui/view/modals/confirm.rs` — `input_modal` (:18), `confirm_modal` (:70), `archive_project_modal` (:134), `remove_project_modal` (:315), `message_modal` (:446), `teardown_modal` (:473).
  - `src/gui/view/modals/settings.rs` — `project_settings_modal` (:26), `tmux_choice_modal` (:30), `agent_picker_modal` (:58), `settings_modal` (:130, archived-projects row at :305, the tmux setting at :325), `shortcut_overlay_modal` (:626-790).
  - `src/gui/view/modals/{theme_picker.rs:17, theme_manager.rs:19,43, archived_projects.rs:23,46, upgrade.rs:16,98}`.
  - `src/gui/add_project.rs` — `open` (:126), `set_path` (:146), `set_name` (:157), `dir_move` (:164), `dir_pick` (:179), `choose_typed` (:196), `choose` (:210), `change_source` (:243), `submit` (:256), `focus_field` (:305), `update` (:318), `handle_key` (:375), `view` (:439).
  - `src/gui/onboarding.rs` (whole file) + `src/gui/update/onboarding.rs` (whole file, incl. `Modal::TmuxChoice` handoff at :97).
  - `src/gui/scripts_editor.rs` — `ScriptsEditorState` (:27-34), `Msg` (:41-61), `open` (:63-77), `save` (:79-107), `update` (:109-139), `view` (:140-335).
  - `src/gui/theme_manager_editor.rs` (whole file — the paste-first editor sub-view and its `handle_key`).
  - `src/gui/session_launcher/` — `mod.rs` (the module map, read it first), `state.rs:13-100` (`LauncherState`'s three list states + the identity-not-index rule), `keys.rs:26+` (`handle_session_launcher_key`), `palette.rs`, `settings.rs`, `theme_panes.rs`, `helpers.rs` (**the pure half: scroll offsets :178-533, ranking :599-628, identity :546-598, row builders :629-745**), `view/{mod,panes,rows,settings_panes,settings_rows}.rs`, and **`tests.rs` (796 lines — the acceptance suite, port it)**.
  - `src/gui/update/pty_input.rs:299-362` — `should_forward` and the `MODAL_OPEN`/`PALETTE_OPEN` statics (:331-337); `escape_should_dismiss` (:364-378). `src/gui/update/mod.rs:280-308` — why the statics exist at all.
  - Modal *entry points*: `src/app/mod.rs:442` (worktree-name `Modal::Input`), `:487-536` (agent picker moves), `:537` + `src/gui/update/mod.rs:775` (Settings), `src/gui/update/mod.rs:558` (ShortcutOverlay), `src/app/spawn.rs:42` (AgentPicker), `src/app/teardown.rs:20` (RemoveProject) and `:187-199` (Teardown), `src/gui/update/sessions.rs:147-177` (`on_run_script`), `src/gui/view/terminal.rs:588` (the run-script button), `src/gui/view/statusbar.rs:145` (shortcuts chip) and `:154` (palette chip), `src/gui/rows.rs:159` (project scripts from a sidebar row).
- **Interfaces Plans 03–07 already shipped — consume them, do not re-derive:**
  - `entities::workspace_state::{WorkspaceState, acknowledge, select_session, visible_order, screen, …}` — the single owner. Modals read and call; they never mutate selection directly.
  - `entities::session_registry::SessionRegistry` — spawning, home terminals, panel shells. The launcher and the agent picker spawn **through it**, never around it.
  - `entities::toast::{ToastState, ToastKind}` — Plan 07 wired two producers (`sessions.rs:482`, `src/app/terminals.rs:104-108`); every remaining producer is a modal in this phase.
  - `entities::activity_store`, `entities::animation_clock`, `entities::project_tree` — untouched.
  - `keymap::{SHORTCUTS, ShortcutDef, Scope, GlobalShortcut, Screen, contexts_for, bindings, binding_for, platform_mod_label}` — **the shortcut overlay is generated from this registry, never hand-written** (spec §5). If the overlay needs a display-only field the ported registry dropped, add it to the registry, not to the overlay.
  - `views::{workspace, sidebar, statusbar, appbar, session_header, terminal_view}`, `theme.rs` token fns, `settings::SettingsState`, `zoom::ZoomState`, `fonts::{UI_FAMILY, MONO_FAMILY}`, `icons::icon`.
- **Carried decisions (do not re-derive, do not re-open):**
  1. **The durable pin is VENDORING, not a fork and not `[patch]`.** The spikes proved a `[patch."https://github.com/zed-industries/zed"]` entry cannot redirect a *same-source* git dependency to a different rev of that same source (findings §S2 "Build note", amendment 2) — the spike only got away with it by patching to a **local path** under `~/.cargo/git/checkouts`, which is garbage-collectable and therefore not durable. A fork is outward-facing (a second repo to own, and unavailable to an offline build). Task 1 vendors the three needed crates into `vendor/gpui-component/`, points their `gpui`/`gpui_macros` at the workspace's pinned ZED_REV, and proves exactly one `gpui` builds.
  2. **The S2 workarounds are law, one per gap, chosen per modal to match iced.**
     - **←/→ (findings §S2 Step 1 row 3, amendment 3):** gpui-component's `MoveLeft`/`MoveRight` are unconditionally bound in the static `"Input"` key context (`movement.rs:139-154`, `state.rs:180-181`) and never propagate. iced's equivalent problem is solved by `should_forward`'s `palette_open` carve-out (`pty_input.rs:353-356`), i.e. **the palette gets the arrows and the caret does not**. Reproduce that with **capture-phase interception** in `ModalInput`: the wrapper intercepts Left/Right before dispatch reaches the `Input` and hands them to the modal when the modal declares `wants_arrows == true`. The alternative workaround (don't-focus-until-typed) is the right one **only** for a modal whose iced counterpart also has no focused field on open; no such modal exists in Grove's palette set, so capture-phase is what ships. Do **not** patch the vendored `movement.rs` — a vendored patch is a fork with extra steps and it would silently diverge from the recorded rev.
     - **Tab (findings §S2 Step 2 row 2, amendment 4):** `IndentInline` always consumes Tab once `multi_line(true)` is set (`indent.rs:57-64,219-252`). The scripts editor's three buffers therefore traverse by **click** (native, free) plus a hand-rolled **non-Tab chord** (`ctrl-tab`) wired at the modal level. Tab inside a multiline buffer indents — say so in the modal's footer hints. The **Onboarding** wizard's Tab focus alternation (`modals.rs:296-308`) is unaffected: those are *single-line* fields, where `tab_index`/`tab_stop` works (`state.rs:464`), but Tab is still bound in the `"Input"` context, so route it the same way as the arrows via `ModalInput`.
     - **Escape is the one contract that already works.** `InputState::escape()` calls `cx.propagate()` (`state.rs:1666`) unless `clean_on_escape` is set — so **never set `clean_on_escape`**, and Escape reaches the modal layer from inside a focused field exactly as `should_forward`'s unconditional Escape carve-out does today (`pty_input.rs:349-352`).
  3. **`should_forward`, `MODAL_OPEN` and `PALETTE_OPEN` do not get ported — they get *deleted* by construction** (spec §5; the Plan 04 deviation-7 debt is settled here). Their three carve-outs map onto gpui as: Escape → carried decision 2 (propagation, already correct); global-mods chords while a modal is open → the modal's own key context binds them, so they are actions and never text (findings §S2 Step 1 row 4); ←/→ while the palette is open → `ModalInput`'s capture-phase interception. Write that mapping into `src/modal.rs`'s module doc **and** assert it in the keyboard matrix; a reviewer must be able to see all three carve-outs survive without either static.
  4. **Single slot, replace-don't-stack, quirks included.** `set_modal` (`modals.rs:645-651`) unconditionally clears **all four** `Grove`-owned child-state fields before repointing the slot, and `open_child` (:669-676) exists purely so a child that only owns `&mut App` cannot forget to. Reproduce that as a *type* property, not a discipline: the gpui `Modal` slot owns its per-modal state inline, so replacing the slot drops the old state and forgetting is impossible. Preserve the two documented quirks verbatim: the **quit confirm clobbers whatever modal is open and cancelling does not restore it** (`on_close_requested`, :338-366 — spec Appendix A calls this out explicitly as a gap to preserve), and the **changelog overlays Settings** (`upgrade.rs:98`). `cancel_modal`'s Teardown special-case (:677-702 — cancel means "skip the script" or "close", and is a no-op mid-removal) is part of the machine, not of the view.
  5. **Focus on mount.** Every modal that has a focusable field calls `focus()` on mount (`InputState::focus(window, cx)`; gpui-development skill pitfall — a field that is never focused silently eats nothing and looks broken). Every modal *without* a field focuses its own root so Escape and its letter keys have somewhere to land. Assert it: opening a modal and reading back the focused handle is a unit-testable property with `VisualTestContext`.
  6. **The palette's pure half goes in `src/launcher.rs` and is TDD'd before any view exists.** `src/gui/session_launcher/helpers.rs` plus `tests.rs` are 1,500 lines of pure, already-tested logic; they port with their tests, exactly like Plan 07's `grid.rs`. Nothing in `src/launcher.rs` may touch gpui.
  7. **Live project-theme preview needs the one terminal-element hook already stubbed for it** (`crates/grove-gpui/src/terminal_element.rs:156`: "Plan 08: launcher/picker live preview — `Some(None)` will mean…"). Wire that hook; do not invent a second theme-override path. Both the launcher's theme pane and the ThemePicker drive it.
- **Recorded ambiguities, resolved by reading the oracle:**
  1. **Onboarding is not a modal-layer modal.** `view()` short-circuits and renders it full-viewport with no sidebar/statusbar/scrim (`view/modals/mod.rs:107-110`, `unreachable!("onboarding short-circuits in view()")`). Model it as a `Modal` variant for the state machine's benefit but render it as a **screen replacement**, and keep the entrance animation (spec §4: `with_animation`). This matches the user's recorded onboarding-mock preference (full-viewport).
  2. **`Modal::Input` is single-purpose today.** Its doc says "today only the worktree-name input" (`modal.rs:7`). Build it as the generic single-field prompt it is (title + buffer + inline red `note` cleared on next edit), because add-worktree, init-and-add and the theme-manager rename all reuse the shape — but do not invent extra call sites.
  3. **`RemoveProject` is two modals in one variant** (`modal.rs:29-40`): a confirm stage with the "also delete worktrees on disk" checkbox, then a progress stage (`in_progress`/`done`/`current`/`errors`) driven by `advance_remove_project` (`modals.rs:857-906`) with its own key handler (`handle_remove_project_key`, :69-93) that refuses to cancel while busy. Keep it one variant; the async drive becomes a `cx.spawn` task feeding the entity, not a tick.
  4. **The archive gate recomputes after every kill** (`refresh_archive_gate`, `modals.rs:736-745`) and its row list is deliberately **not** filtered to running sessions (`archive_gate_sessions`, :703-720) so the gate's count can never disagree with what `kill_sessions_for_project` would kill. Port the comment.
  5. **Settings persists immediately; there is no apply/cancel footer** (`modal.rs:112-115`). The two rows Plan 06/07 explicitly deferred here are the **archived-projects** row (`settings.rs:305`, which opens `Modal::ArchivedProjects`) and the **tmux** setting (`settings.rs:325`, `App::use_tmux()`); both are checklist rows.
  6. **The shortcut overlay is registry-generated plus exactly two static rows** — copy/paste and "Close modals" (`settings.rs:665-669`) — and it closes on Escape **or** on its own chord (`modals.rs:301-308`). It also filters by the current screen (`scope_allows`), so it is screen-sensitive; test that.
  7. **The agent-session toast producer Plan 07 could not reach lands here.** `"failed to start session: {e}"` (`sessions.rs:482`) belongs in `Sidebar::spawn_session`, which this phase touches because the launcher and the agent picker both spawn through it.
- No `git` commands until Task 7. Do not commit intermediate tasks. The orchestrator runs `./install.sh` and the commit.

---

### Task 1: The durable pin — vendor gpui-component

**Files:**
- Create: `vendor/gpui-component/{README.md,LICENSE-APACHE,ui/,macros/,assets/}`
- Modify: root `Cargo.toml`, `crates/grove-gpui/Cargo.toml`, `.gitignore` if it would swallow `vendor/`

**Interfaces:**
- Produces: a `gpui-component` that builds offline, at a recorded rev, against the workspace's own pinned gpui — and a proof that exactly one `gpui` exists in the graph.

- [ ] **Step 1: Decide the crate set by reading, then copy**

The upstream workspace has six crates (`ui`, `macros`, `assets`, `story`, `story-web`, `webview`). Only three are reachable from `gpui_component::input`:
- `ui` — the monolith that contains `Input`/`InputState`/the editor. **It cannot be trimmed further**: the input lives inside the same crate as every other component and has no feature gate. Its `[features]` default set is **empty** (no `default = [...]` key), so tree-sitter, decimal, inspector and the 30+ grammar deps stay off unless we enable them. Enable **nothing**.
- `macros` — `gpui-component-macros`, a `[dependencies]` entry of `ui`.
- `assets` — `gpui-component-assets`, a `[dependencies]` entry of `ui` (its own `assets/` folder is ~400 KB of icon SVGs; it is embedded via `rust-embed` in a `build.rs`).

`story`, `story-web`, `webview` and every `examples/*` are **not** copied — they pull `reqwest`, `wasm-bindgen` and a WebView stack we must never link.

```bash
cd /home/gitfudge/dev/gitfudge0/grove
SRC=~/.cargo/git/checkouts/gpui-component-95ce574d8a0da8b8/88f102d
mkdir -p vendor/gpui-component
cp -r "$SRC/crates/ui"     vendor/gpui-component/ui
cp -r "$SRC/crates/macros" vendor/gpui-component/macros
cp -r "$SRC/crates/assets" vendor/gpui-component/assets
cp    "$SRC/LICENSE-APACHE" vendor/gpui-component/LICENSE-APACHE
find vendor/gpui-component -name '*.rs.orig' -o -name 'target' -type d | xargs -r rm -rf
```

If that checkout is missing, re-fetch it by adding the git dependency at GPUI_COMPONENT_REV once, building, then copying — **never** copy from a different rev.

- [ ] **Step 2: Write the provenance README (this is the pin)**

`vendor/gpui-component/README.md` must record, in plain text a future reader can act on: upstream `https://github.com/longbridge/gpui-component`, rev `88f102d13654fe25aa2fede076274b6b751a3704`, upstream version `0.5.2`, license **Apache-2.0** (with `LICENSE-APACHE` beside it), the exact `cp` commands above, the three-crate subset **and why** (`ui` is unsplittable; `story`/`webview` excluded), the statement that **the tree is unmodified except for the manifest edits in Step 3** (list them), and the reason vendoring was chosen over `[patch]` (impossible for a same-source git rev) or a fork (outward-facing, offline-hostile). Any future edit to the vendored source must be appended to that list or it is invisible.

- [ ] **Step 3: Re-point the manifests, and keep them out of the workspace**

Upstream's manifests use `workspace = true` for ~20 dependencies against **their** workspace, which no longer exists here. Resolve each one to a concrete version — read the upstream root `Cargo.toml`'s `[workspace.dependencies]` for the exact specs, do not guess — with three specific rules:
- `gpui` and `gpui_macros` become **our** workspace's pinned entries: `git = "https://github.com/zed-industries/zed", rev = "<ZED_REV>"`. This is the whole point of the exercise: upstream pins `gpui` with **no rev** and floats onto zed's default branch (findings §S2 "Build note"), which is what broke the spike.
- `edition` comes from upstream's `[workspace.package] edition = "2024"` — write `edition = "2024"` explicitly in each vendored manifest. rustc 1.95 supports it; our own crates stay on 2021.
- Drop `publish`, `version.workspace`, `[package.metadata.cargo-machete]` and any `[lints]` inheritance.

Then, in the **root** `Cargo.toml`, add the three paths to `[workspace] exclude` (not `members`). A path dependency is otherwise auto-promoted to a workspace member, which would subject vendored code to our `[lints]` and to `--all-targets` clippy. Add `gpui-component = { path = "vendor/gpui-component/ui" }` to `[workspace.dependencies]`, and `gpui-component.workspace = true` to `crates/grove-gpui/Cargo.toml`, replacing its "NO gpui-component in this phase" comment with a one-line pointer to `vendor/gpui-component/README.md`.

- [ ] **Step 4: Build-parity check (the actual deliverable)**

```bash
cd /home/gitfudge/dev/gitfudge0/grove
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 build -p grove-gpui 2>&1 | tail -20
# exactly ONE gpui, at our rev — not two, not a floating edge:
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 tree -p grove-gpui -i gpui 2>&1 | head -30
grep -n 'name = "gpui"' -A 3 Cargo.lock
# the default toolchain must be untouched:
cargo build 2>&1 | tail -5
git status --short src crates/grove-core crates/grove-terminal   # expect EMPTY
```

Expected: the build is green; `Cargo.lock` contains **one** `gpui` entry and its `source` carries ZED_REV; `cargo tree -i gpui` shows both `grove-gpui` and `gpui-component` depending on that same one. **If two `gpui`s appear, STOP and report** — the vendored manifests still float somewhere, and no amount of downstream code fixes that.

Also record, in the plan-completion note: the vendored tree's size, the wall-clock cold build time it adds, and whether `assets`' `build.rs` needed anything (it uses `rust-embed` with `interpolate-folder-path`).

- [ ] **Step 5: Smoke-test the two S2 gaps against the vendored copy**

Before writing a single modal, prove the workarounds are still needed and still possible at *this* rev, in *our* tree — grep the vendored source, do not trust the findings doc alone:

```bash
grep -n "fn left\|fn right" vendor/gpui-component/ui/src/input/movement.rs | head
grep -n "MoveLeft\|MoveRight\|IndentInline" vendor/gpui-component/ui/src/input/state.rs | head
grep -n "fn is_indentable" -A 8 vendor/gpui-component/ui/src/input/indent.rs
grep -n "cx.propagate" vendor/gpui-component/ui/src/input/state.rs | head
```

Expected: `left()`/`right()` still call `move_to` unconditionally; `IndentInline` is still bound to bare `tab`; `is_indentable()` is still true for multiline; `escape()` still propagates. Report any divergence from findings §S2 as a **contradiction** before proceeding.

---

### Task 2: The modal state machine, the layer, and the input wrapper (TDD)

**Files:**
- Create: `crates/grove-gpui/src/modal.rs`, `crates/grove-gpui/src/views/modals/{mod.rs,shell.rs,input.rs}`
- Modify: `crates/grove-gpui/src/views/workspace.rs`, `crates/grove-gpui/src/keymap.rs`, module lists

**Interfaces:**
- Produces: `Modal` (the slot), `ModalLayer`, the shared modal chrome, `ModalInput`, the per-modal key contexts, and the Escape/confirm-kill carve-outs that finally retire `should_forward`.

- [ ] **Step 1: The pure slot (tests before the type)**

`src/modal.rs`, no gpui. Port `src/app/modal.rs:5-186`'s variant set (one variant per modal, carrying exactly the state that variant needs) plus the lifecycle from `modals.rs:645-702`. Mandatory invariants, each a named test:
- opening a modal while one is open **replaces** it and drops the old state (carried decision 4) — there is no stack and no restore;
- the **quit confirm clobbers** any open modal, and cancelling it leaves `None`, not the clobbered modal (`on_close_requested`, `modals.rs:338-366`);
- `cancel` on `Teardown` does **not** close: it skips a running script, closes only once the stage is `Done`, and is a **no-op** mid-`Removing` (`modals.rs:677-702`);
- `cancel` on `RemoveProject` while `in_progress` is refused (`handle_remove_project_key`, :69-93);
- the changelog can overlay `Settings` and dismissing it returns to `Settings` (`upgrade.rs:98`), while the theme picker's `return_to_settings` does the same for its own round trip (`modal.rs:79-82`);
- `ScriptsEditor → ThemePicker → ScriptsEditor` preserves the editor's buffers (the documented `open_child` exception, `modals.rs:660-668`).

- [ ] **Step 2: The per-modal keyboard verdicts (the matrix's spine)**

Still pure. Port `handle_modal_key` (`modals.rs:94-336`) as a **table**: `(ModalKind, Key, Modifiers) -> ModalKeyVerdict`, where the verdict is one of `Close`, `Submit`, `Move(i32)`, `Custom(..)`, `Ignore`, `FallThrough`. Every arm in that function is a row; do not summarize it. The per-modal asymmetries are the point and each needs its own test:
- `Confirm`: Escape=no, Enter=yes, `y`/`n` (`:135-145`);
- `ArchiveProject`: Escape/`n` cancel, `y` routes through the gate re-check so it cannot bypass a disabled button (`:148-160`);
- `Message`: Escape **or** Enter **or** `q` (`:165-169`);
- `TmuxChoice`: Enter/`t`/`y` = on, `n` = off, **Escape dismisses without persisting** so the choice is re-asked next launch (`:258-269`);
- `ThemePicker`: arrows + `j`/`k`, Tab **and** `h`/`l` switch tabs (`:171-184`);
- `ThemeManager`: three nested sub-states (editor open → delegate; `pending_delete` → y/n/Enter/Escape; `rename` → Enter/Escape; else list) (`:186-228`);
- `AgentPicker`: Space toggles default (`:230-241`);
- `Updating`: Escape closes **only when not mid-update** (`:250-256`);
- `ShortcutOverlay`: Escape **or its own registry chord** (`:301-308`);
- `Onboarding`: Escape skips, Tab alternates path/name focus on the Project step only (`:270-300`);
- `Settings`, `ArchivedProjects`, `ScriptsEditor`, `Teardown`: Escape only (`:245-249,161-164,309-335`).

`escape_should_dismiss` (`pty_input.rs:364-378`) ports here too — it is the **no-modal-open** half of the same question, and it is what makes Escape reach the PTY when nothing is armed. The two-step **confirm-kill** arming/disarming (spec §5, spec Appendix A *Shortcuts*) is one of its four inputs and stays a `WorkspaceState` concern; the modal machine only asks whether a modal outranks it.

- [ ] **Step 3: `ModalInput` — both S2 workarounds, in one place (carried decision 2)**

`views/modals/input.rs`. A thin wrapper owning a `gpui_component::input::InputState` plus a small policy struct:
- `wants_arrows: bool` — when set, Left/Right are intercepted **before** dispatch reaches the `Input` and are re-emitted to the hosting modal (the palette's `PALETTE_OPEN` carve-out, `pty_input.rs:353-356`). When clear, the caret gets them, exactly as iced's non-palette fields do.
- `wants_tab: bool` — same mechanism for Tab, used by Onboarding's field alternation. Multiline buffers leave it clear and Tab indents.
- **Never** `clean_on_escape`: Escape must keep propagating to the layer (findings §S2, `state.rs:1666`).
- `focus(window, cx)` on mount, and `set_selected_range(len..len, cx)` for the move-cursor-to-end idiom (`modals.rs:625-644`'s `focus_add_project_field` does exactly this in iced).

Write the interception with the gpui-development skill open — get the capture/bubble phase right at *this* rev and record the API you used in the module doc, because a future reader cannot re-derive it from training data. Unit-test the policy struct (which keys a given modal claims) even though the dispatch itself needs a window.

- [ ] **Step 4: The layer and the shared chrome**

`views/modals/mod.rs`: one `ModalLayer` entity holding the single slot, rendered by `Workspace` **above** everything, with the `SCRIM()` full-bleed centering container (`view/modals/mod.rs:139-149`) and the two exceptions — Onboarding replaces the screen (recorded ambiguity 1) and the launcher top-drops rather than centering (`view/modals/mod.rs:114-121`). The layer sets a **per-modal `key_context`** (spec §4: "each modal is its own entity with its own key-context string") and focuses on mount (carried decision 5).

`views/modals/shell.rs`: port the twelve helpers from `src/gui/widgets/modal.rs` listed in the oracles. These are pure presentation and every later task depends on them; build them once, correctly.

- [ ] **Step 5: Retire `should_forward` by construction (carried decision 3)**

In `src/modal.rs`'s module doc, write the three-way mapping (Escape / global-mods chords / palette arrows) and state that neither `MODAL_OPEN` nor `PALETTE_OPEN` has a gpui counterpart. Bind the global-mods chords in each modal's own key context so they arrive as actions, never as text. Add the drift guard: **a test that fails if any modal's context binds a chord that the modal's verdict table does not claim**.

- [ ] **Step 6: Verify**

```bash
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 test -p grove-gpui modal 2>&1 | tail -40
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 clippy -p grove-gpui --all-targets --no-deps -- -D warnings 2>&1 | tail -20
```

---

### Task 3: The simple wave — Confirm, Message, Input, TmuxChoice, AgentPicker, and the destructive trio

**Files:**
- Create: `crates/grove-gpui/src/views/modals/{confirm.rs,project.rs}`
- Modify: `crates/grove-gpui/src/views/{sidebar,session_header,workspace}.rs`

**Interfaces:**
- Produces: nine modals with no free-form text beyond one single-line field, every sidebar row action wired, the run-script button, and the agent-session toast producer.

- [ ] **Step 1: Confirm (incl. Quit), Message, Input** (`view/modals/confirm.rs:18-133,446-472`)

`confirm_modal` with its `destructive` styling and `ConfirmKind` payloads (`modal.rs:177-186`): `RemoveProject`, `RemoveWorktree`, `InitAndAddWorktree`, `Quit`. `input_modal` is the single-field prompt (recorded ambiguity 2) — title, `ModalInput` with `wants_arrows == false`, inline red `note` cleared on the next edit (`app/mod.rs:557-563`), Enter submits (`submit_modal_input`, `modals.rs:541-557`), Ctrl+C cancels (`:100-104`). `message_modal` is text + one dismiss.

The **Quit** path is the close-request interception (spec §7, `modals.rs:338-366`): running **native** sessions confirm, tmux-backed ones do not. Wire it to gpui's should-close callback; `flush_ui_zoom_save` on every exit path is Plan 09's, but note in the code where it hooks.

- [ ] **Step 2: TmuxChoice and AgentPicker** (`view/modals/settings.rs:30-129`, `app/spawn.rs:42`, `app/mod.rs:487-536`)

TmuxChoice is a two-button dialog whose Escape deliberately persists nothing (Task 2 Step 2). AgentPicker lists `available_agents` with a selection cursor, Space toggling "make this the default", Enter spawning through `SessionRegistry`.

- [ ] **Step 3: RemoveProject + ArchiveProject + ArchivedProjects** (`confirm.rs:134-314,315-445`, `archived_projects.rs:23-158`)

RemoveProject's two stages in one variant (recorded ambiguity 3): the checkbox, then the progress view driven by a `cx.spawn` task that reports `done`/`current`/`errors` back into the entity — **not** a tick, and **not** blocking the frame. Port `kick_off_remove_project`/`advance_remove_project` (`modals.rs:797-906`) faithfully, including that cancel is refused while busy.

ArchiveProject is the blocking gate: one row per **session** (never per worktree), unfiltered by liveness, recomputed after every kill (recorded ambiguity 4, `modals.rs:703-745`). ArchivedProjects is a marker modal deriving every row live from `store.archived_projects()` (`modal.rs:56-59`) with restore/delete per row.

- [ ] **Step 4: Teardown, with its embedded live PTY** (`confirm.rs:473-553`, `app/teardown.rs:187-199`)

The only modal that hosts a terminal. Reuse `TerminalView` for the teardown script's session — do **not** write a second terminal renderer — and drive `TeardownStage` (`RunningScript → Removing → Done{failed}`, `modal.rs:152-161`), remembering `removal_started` exists so a `Removing` frame paints **before** the UI blocks on `git worktree remove` (`modal.rs:171-174`). In gpui the blocking removal belongs on the background executor, which makes that flag a paint-ordering detail rather than a hack — record that in a comment either way.

- [ ] **Step 5: Wire the entry points and the toast producer**

Replace the sidebar stubs (`views/sidebar.rs:228-232,617`): `AddWorktree` → `Modal::Input`, `DeleteWorktree` → confirm → Teardown, `RemoveProject` → RemoveProject, `ProjectScripts` → ScriptsEditor (Task 6 fills the view; open the slot now), `RunScript` → `on_run_script`'s behavior (`sessions.rs:147-177` — opens the panel, or appends a tile in grid view), `AddProject` → Task 4's wizard. Same for `session_header`'s `run script` (`views/workspace.rs:431`, oracle `terminal.rs:588`).

Then **recorded ambiguity 7**: `ToastState::set_error("failed to start session: {e}")` in `Sidebar::spawn_session` (oracle `sessions.rs:482`) — Plan 07 could not reach it because it never touched that function.

- [ ] **Step 6: Verify**

```bash
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 test -p grove-gpui 2>&1 | tail -40
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 clippy -p grove-gpui --all-targets --no-deps -- -D warnings 2>&1 | tail -20
```

---

### Task 4: The two wizards — AddProject and Onboarding

**Files:**
- Create: `crates/grove-gpui/src/views/modals/{add_project.rs,onboarding.rs}`

**Interfaces:**
- Produces: the two-step add-project wizard with directory autocomplete, and the full-viewport first-run wizard.

- [ ] **Step 1: The add-project pure half (tests first)**

Port from `src/gui/add_project.rs`: the state shape (`:54+`), `set_path`/`set_name` (:146-163), `dir_move` (:164-178), `dir_pick` (:179-195), `choose_typed` (:196-209), `choose` (:210-242), `change_source` (:243-255) and `submit` (:256-304) with its validation notes and git probe. The directory match list is a pure function of the typed path — test it against a temp tree, never against `$HOME`.

- [ ] **Step 2: The wizard view and its keys** (`add_project.rs:439+`, `modals.rs:117-136`)

Two steps (`PickSource`, details). The path field is a `ModalInput`; arrows drive the **match list**, not the caret, on the pick-source step — that is a `wants_arrows` modal. Escape cancels from pick-source; Ctrl+C cancels from either step (`modals.rs:120-131`). The folder-browse path uses `rfd`/`prompt_for_paths` with the `picker_open` guard (spec §7, `modals.rs:490-534`).

- [ ] **Step 3: Onboarding** (recorded ambiguity 1; `src/gui/onboarding.rs`, `src/gui/update/onboarding.rs`)

Full-viewport, no scrim, no sidebar. Steps mirror `OnboardStep`; the project step reuses Step 1's autocomplete; the session step picks an agent and the permissions choice (`perms_skip`, preselected safe); finishing persists an explicit store value and hands off to `Modal::TmuxChoice` (`update/onboarding.rs:97`). **Tab alternates path/name focus** (`modals.rs:296-308`) — single-line fields, so `wants_tab` (carried decision 2). The entrance animation maps to `with_animation` (spec §4).

- [ ] **Step 4: Verify**

```bash
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 test -p grove-gpui 2>&1 | tail -40
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 clippy -p grove-gpui --all-targets --no-deps -- -D warnings 2>&1 | tail -20
```

---

### Task 5: The palette — SessionLauncher and ThemePicker

**Files:**
- Create: `crates/grove-gpui/src/launcher.rs`, `crates/grove-gpui/src/views/modals/{launcher.rs,theme_picker.rs}`
- Modify: `crates/grove-gpui/src/{terminal_element.rs,views/statusbar.rs,views/workspace.rs}`

**Interfaces:**
- Produces: the recents-first command palette with its drill-ins and live theme preview, the theme picker, the palette chip, and the three launcher actions.

- [ ] **Step 1: `src/launcher.rs` — the pure half (carried decision 6, TDD)**

Port `src/gui/session_launcher/helpers.rs` wholesale: scroll offsets (`:178-533`), `reselect_setting` (:504), `nth_session_row` (:515), `cycle_agent` (:527), `agent_sel_for` (:540), `row_identity`/`resolve_row_by_identity` (:546-598), `rank_and_group_combos` (:599-628), `root_project_order` (:629-649), `next_theme_mode` (:650-673), `update_available_actions` (:701-716), `switch_terminal_rows`/`merge_switch_rows` (:717-737), `check_updates_opens_strip` (:738-745). Bring **`src/gui/session_launcher/tests.rs` (796 lines)** across as the acceptance suite; a test that no longer applies must be deleted **with a recorded reason**, never silently dropped.

The load-bearing invariant, from `state.rs:28-48`: **selection is resolved by row *identity*, never by index** — a query edit or a recency re-sort must not activate a different row. Test it.

- [ ] **Step 2: The palette view and its three list states** (`state.rs:13-58`, `view/{mod,panes,rows}.rs`)

Root (recents + actions), typing/browse-all (every project×worktree combo, fuzzy-filtered), and the drill-ins: `switch`, `row_actions` (the Tab-revealed strip with its agent icon bar), `settings`. Top-dropped, not centered (`view/modals/mod.rs:114-121`). The search field is the canonical `wants_arrows` modal (carried decision 2): ←/→ mean *agent cycling / zoom / update-strip*, never caret movement, and Cmd/Ctrl chords are actions, never text.

- [ ] **Step 3: The settings drill-in** (`session_launcher/settings.rs`, `view/settings_panes.rs`, `view/settings_rows.rs`, `src/gui/update/settings_rows.rs`)

The scoped settings list plus the backend / permissions / default-agent / app-size panes, each with its own commit path. `SettingRow`'s label/icon/section/is_toggle table (`update/settings_rows.rs:34-106`) ports with its two tests (:107-135).

- [ ] **Step 4: ThemePicker + the live preview hook (carried decision 7)**

`view/modals/theme_picker.rs:17+`: dark/light tabs, follow-system checkbox, app-vs-project scope, `return_to_settings` round trip, `project_use_default`. Both the picker and the launcher's theme panes (`session_launcher/theme_panes.rs`) drive the **live preview** through the single stubbed hook at `crates/grove-gpui/src/terminal_element.rs:156` — wire that hook; do not add a second override path. Cancelling restores `original` (`modal.rs:74-94`).

- [ ] **Step 5: Actions and the chip**

`NewSession`, `NewSessionInWorktree` and `SwitchSession` stop being stubs (`views/workspace.rs:1228-1230`) and open the palette in the right state; the appbar `+` segment and the statusbar **palette chip** (`view/statusbar.rs:154`) do the same. Spawning goes through `SessionRegistry`/`Sidebar::spawn_session`, so Task 3's toast producer covers failures.

- [ ] **Step 6: Verify**

```bash
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 test -p grove-gpui launcher 2>&1 | tail -40
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 test -p grove-gpui 2>&1 | tail -20
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 clippy -p grove-gpui --all-targets --no-deps -- -D warnings 2>&1 | tail -20
```

---

### Task 6: The editors — Settings, ShortcutOverlay, ScriptsEditor, ThemeManager, Updating

**Files:**
- Create: `crates/grove-gpui/src/views/modals/{settings.rs,shortcuts.rs,scripts_editor.rs,theme_manager.rs,upgrade.rs}`
- Modify: `crates/grove-gpui/src/views/{statusbar,appbar,workspace}.rs`

**Interfaces:**
- Produces: the last five modals, the cog, the shortcuts chip, and the multiline-editor traversal contract.

- [ ] **Step 1: Settings** (`view/modals/settings.rs:130-625`, recorded ambiguity 5)

Every control **persists immediately**; there is no apply/cancel footer. Port the sections verbatim, and specifically the two rows deferred here: **archived projects** (`:305`, opens `Modal::ArchivedProjects` — Task 3 built it) and the **tmux** setting (`:325`, `App::use_tmux()`). The cog (`views/appbar.rs:216`) opens it; the `Settings` action (`views/workspace.rs:1231`) does too. The upgrade dot on the cog stays Plan 09's stub.

- [ ] **Step 2: ShortcutOverlay, from the registry** (recorded ambiguity 6, `settings.rs:626-790`)

Generated from `keymap::SHORTCUTS` — filtered by the current screen via `scope_allows`, grouped when the visible set spans Global **and** screen scopes, with the alt-chord label rule (`cmd+alt+n` / `ctrl+alt+n`, never `ctrl+shift+alt+n`), the `literal` display rows, the macOS ⌘-SVG substitution, and exactly two static rows (copy/paste, "Close modals"). Closes on Escape **or** its own chord. Wire the statusbar **shortcuts chip** (`view/statusbar.rs:145`) and the `ShortcutOverlay` action (`views/workspace.rs:1232`).

Add the drift guard: **every registry row with a display label appears in the overlay for at least one screen** — the registry is the single source of truth (spec §5) and this test is what proves it stayed that way.

- [ ] **Step 3: ScriptsEditor — three multiline editors** (`src/gui/scripts_editor.rs`, carried decision 2)

Three `InputState::new(..).multi_line(true)` buffers seeded from the project's setup/run/teardown (`:63-77`), saved with the empty→`None` normalization and the save-failure `Message` modal (`:79-107`). **Traversal is click plus `ctrl-tab`**, never Tab — Tab indents inside a buffer; say so in the footer hints. The footer also carries "Project theme" (opens the ThemePicker, preserving the buffers — the documented `open_child` exception) and "Archive project" (opens Task 3's gate).

- [ ] **Step 4: ThemeManager + its editor** (`view/modals/theme_manager.rs`, `src/gui/theme_manager_editor.rs`)

The list sub-view (per-row rename/duplicate/delete/edit, "New theme", the swatch strip) with its three nested key sub-states from Task 2's table, plus the paste-first multiline editor sub-view — a fourth multiline `Input`, same traversal contract as Step 3. Editing a theme invalidates the PTY render path (`modals.rs:214-219`).

- [ ] **Step 5: Updating + changelog shells** (`view/modals/upgrade.rs:16-97,98-182`)

Render whatever `UpgradeState` reports today; Escape closes only when not mid-update (Task 2 Step 2). The changelog **overlays Settings** and returns to it (carried decision 4). Plan 09 owns the live stages, the fetch and apply/restart — leave a one-line pointer, not a stub that lies about being done.

- [ ] **Step 6: Verify**

```bash
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 test -p grove-gpui 2>&1 | tail -40
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 clippy -p grove-gpui --all-targets --no-deps -- -D warnings 2>&1 | tail -20
```

---

### Task 7: Verification, the keyboard matrix, and the manual checklist

**Files:**
- Modify: `docs/superpowers/plans/2026-07-31-gpui-rewrite-00-master.md` (row 08 → done)

**Interfaces:**
- Produces: the phase's evidence, and the exit gate's second half.

- [ ] **Step 1: The keyboard matrix, as an automated table test** (spec §8.2 — half the exit gate)

One table-driven test covering **every `SHORTCUTS` row × {Workspace, Grid, Zen, each modal} × armed/disarmed**, asserting the dispatch target: PTY bytes, a named action, or swallowed. It must pin, by name, the observable contracts spec §5 enumerates:
- **Escape despite capture** — Escape reaches the modal from inside a focused `ModalInput`, and with no modal open reaches the PTY unless `escape_should_dismiss` says otherwise;
- **palette arrow carve-outs** — ←/→ reach the palette, not the caret, and only while the palette is open;
- **Cmd-chord suppression in the palette** — a global-mods chord is an action, never inserted text;
- **Alt+Escape reaches the PTY as `ESC ESC`**;
- **two-step confirm-kill arming/disarming**, sessions and home terminals armed separately, Escape disarming;
- **fall-through to PTY** for any screen-scoped row whose screen is not current.

Then the drift guards from Tasks 2 and 6 (no context binds an unclaimed chord; no registry row is missing from the overlay).

```bash
cd /home/gitfudge/dev/gitfudge0/grove
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 test -p grove-gpui keyboard_matrix 2>&1 | tail -40
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 build -p grove-gpui 2>&1 | tail -5
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 test -p grove-gpui 2>&1 | tail -40
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 clippy -p grove-gpui --all-targets --no-deps -- -D warnings 2>&1 | tail -20
GROVE_CONFIG_DIR=$(mktemp -d) GROVE_GPUI_SELFTEST=1 PATH="$HOME/.cargo/bin:$PATH" \
  cargo +1.95.0 run -p grove-gpui 2>&1 | tail -5
# the rest of the workspace, DEFAULT toolchain — must be untouched
rustc --version
cargo build 2>&1 | tail -5
cargo test 2>&1 | tail -10
git status --short src crates/grove-core crates/grove-terminal   # expect EMPTY
rustfmt --edition 2021 --check crates/grove-gpui/src/*.rs crates/grove-gpui/src/*/*.rs crates/grove-gpui/src/views/modals/*.rs
grep -rn "MODAL_OPEN\|PALETTE_OPEN\|should_forward" crates/grove-gpui/src --include=*.rs
```

Expected: everything green; the Plan 03 metric selftest still prints its `cell_w=7.5… OK` line; the final `grep` returns hits **only** inside doc comments explaining why those three no longer exist (carried decision 3); `git status` reports no changes under `src/`, `crates/grove-core/`, `crates/grove-terminal/`; `rustfmt --check` was never pointed at `vendor/`.

- [ ] **Step 2: MANUAL — the spec Appendix A *Modals* rows (human, real desktop)**

```bash
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 run --release -p grove-gpui
# and, side by side, the installed iced build:
~/.local/bin/grove
```

Report each row pass/fail. **Do not claim any of these yourself.** Rows 1–19 are spec Appendix A's *Modals* clause, verbatim and in order; 20–23 are the cross-cutting contracts that paragraph's preamble names.

1. **Input** — the worktree-name prompt: focused on open, Enter submits, Escape and Ctrl+C cancel, an invalid name shows the inline red note and the note clears on the next keystroke.
2. **Confirm (incl. Quit)** — Escape=no / Enter=yes / `y` / `n`; destructive styling on the destructive kinds; closing the window with running **native** sessions confirms first, tmux-backed ones quit straight away.
3. **AddProject wizard w/ dir autocomplete** — typing a partial path lists matching directories, ↑↓ walks them, Enter/Tab picks; the browse button opens the OS picker once (no double-open); step two probes for a git repo and offers init; Escape from step one cancels, Ctrl+C cancels from either.
4. **RemoveProject w/ async teardown progress** — the checkbox controls whether worktrees are deleted on disk; the progress view counts up with the current path, collects per-worktree errors, and refuses to cancel while busy; the window stays responsive throughout.
5. **ArchiveProject + ArchivedProjects** — the gate lists one row per **session** (not per worktree) with honest running/exited labels, refuses while any live session remains, and re-counts after each kill; the archived list restores and deletes, and a restored project reappears in the tree.
6. **Message** — Escape, Enter and `q` all dismiss.
7. **TmuxChoice** — Enter/`t`/`y` chooses tmux, `n` chooses native, **Escape persists nothing** and the choice is re-asked on the next launch.
8. **AgentPicker** — ↑↓/`j`/`k` move, Space toggles "default agent", Enter spawns; a failing spawn raises the error toast.
9. **SessionLauncher** — recents first; typing filters every project×worktree combo; Tab opens the row-action strip and ←/→ cycle the agent **inside** it while the caret never moves; the switch drill-in lists sessions then home terminals; the settings drill-in reaches all four panes; **live project-theme preview repaints the terminal behind the palette** and cancelling restores the previous theme.
10. **ThemePicker** — dark/light tabs (Tab, `h`/`l`), follow-system, app-vs-project scope, "Default (follow app)" for project scope; Escape restores the original theme; entered from Settings it returns to Settings.
11. **ThemeManager + multiline editor** — rename (with the collision error), duplicate, delete (y/n), new; the paste-first editor accepts a multi-line paste, saves, and re-colors the terminal immediately.
12. **Settings (immediate persist)** — every control writes through with no apply button; the **archived-projects** row opens the archived list; the **tmux** setting flips the backend for the next spawn; reopening Settings shows the persisted values after a restart.
13. **ShortcutOverlay (registry-generated)** — every chord shown matches what the key actually does, the visible set changes with the current screen, alt-chords render as `{mod}+alt+…`, macOS shows ⌘, and the two static rows (copy/paste, "Close modals") are present. Escape **and** the overlay's own chord close it.
14. **Teardown (embedded live PTY)** — the teardown script runs visibly inside the modal, Escape skips it and proceeds to removal, removal cannot be interrupted, and the final state reports success or failure.
15. **ScriptsEditor (3 multiline editors)** — all three buffers edit independently and scroll independently; **Tab indents** and **ctrl-tab / clicking** moves between them; Save persists (an emptied buffer clears that script) and Cancel discards; "Project theme" opens the picker and coming back **preserves unsaved buffers**.
16. **Updating** — the shell renders the current upgrade state and Escape closes it only when no update is in flight (live stages are Plan 09).
17. **Onboarding wizard** — full-viewport with no sidebar/statusbar/scrim, entrance animation, Tab alternates path/name on the project step, the session step's agent + permissions choices persist, and finishing hands off to the tmux choice.
18. **One-deep, replace-don't-stack** — opening a second modal replaces the first; there is no back-stack anywhere.
19. **Quit-confirm clobbers (the preserved gap)** — requesting close while any modal is open replaces it with the quit confirm, and cancelling that confirm leaves **no** modal, not the one it clobbered.
20. **Per-modal Escape semantics** — spot-check the asymmetries: Teardown's Escape means "skip", RemoveProject's is refused while busy, Updating's is refused mid-update, TmuxChoice's persists nothing, everything else closes.
21. **Focus on open** — every modal with a field has the caret in it immediately; every modal without one still responds to its letter keys and Escape with no click first.
22. **Changelog overlays Settings** — opening the changelog from Settings returns to Settings on dismiss.
23. **Toasts from modals** — a failing session spawn from the launcher/agent picker shows `failed to start session: …` in the statusbar and it clears on the error TTL.

Rows explicitly **deferred** and not checked here (record as deferred, not failed): the upgrade flow's live stages, changelog fetch and apply/restart → **Plan 09**; telemetry, quit-path persistence and tmux sidecar reattach → **Plan 09**; the macOS dock badge/bounce and the mac-only ⌘ chord behaviors → **Plan 10 on a macOS host**; the scripted screenshot sweep across every modal × 3 zooms × 4 themes and the idle-power measurement → **Plan 10**; IME composition inside modal fields and the Wayland clipboard round-trip (findings amendments 5/6) → **Plan 10's manual sweep**.

- [ ] **Step 3: `./install.sh`** — the orchestrator runs this.

```bash
./install.sh 2>&1 | tail -20
```

Expected: the release build + install of the **iced** `grove` binary still succeeds, untouched by this phase.

- [ ] **Step 4: Update the master plan and commit** — the orchestrator runs this.

Mark row 08 `done` in `docs/superpowers/plans/2026-07-31-gpui-rewrite-00-master.md` with a one-line note recording: whether any grove-core amendment had to be authorized (expected: none); **the vendoring outcome** — which crates were copied, the manifest edits made, the `cargo tree -i gpui` result (one gpui or two), tree size and added build time; whether the Task 1 Step 5 grep confirmed both S2 gaps still exist at the vendored rev; which S2 workaround each text-heavy modal actually shipped; that `should_forward`/`MODAL_OPEN`/`PALETTE_OPEN` have no counterpart in grove-gpui and the keyboard matrix proves the three carve-outs survive; and any Appendix A modal row that came back FAIL or MANUAL-deferred.

```bash
git add vendor crates/grove-gpui Cargo.toml Cargo.lock docs/superpowers/plans/2026-07-31-gpui-rewrite-00-master.md
git commit -m "feat(gpui): modal layer and all modals, on a vendored gpui-component"
```

**Exit gate met when:** the Appendix A *Modals* rows above are signed off by a human as pass or explicitly-deferred, the keyboard matrix test is green with the six named contracts pinned (raw output pasted), both drift guards pass, `cargo tree -i gpui` shows exactly one gpui at ZED_REV, grove-gpui builds/tests/clippy clean on 1.95, the iced app and both existing crates are provably untouched and still build on the default toolchain, and `./install.sh` is green.
