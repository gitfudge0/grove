# gpui Rewrite — Master Plan Index

> **For agentic workers:** This is an index, not an executable plan. Execute the numbered phase plans in order; each is a standalone plan document following superpowers:writing-plans. Later plans are written just-in-time when their phase starts, incorporating findings from earlier phases.

**Spec:** `docs/superpowers/specs/2026-07-31-gpui-rewrite-design.md` (the authority for all behavior; Appendix A is the acceptance checklist)

**Branch:** all work happens on `gpui-rewrite` (big-bang branch off `main`).

| # | Plan | Exit gate | Status |
|---|------|-----------|--------|
| 01 | Spikes (terminal element, text inputs, zoom, Linux matrix) | Findings doc committed; go/no-go + rev pins decided | done — findings committed; awaiting user go/no-go |
| 02 | `crates/grove-terminal` + dual-parser golden harness | Golden tests green against vt100 oracle | done — 8/8 golden tests green on rustc 1.94.1 (default toolchain, no `rust-toolchain.toml` needed). Two asserted divergences amend spec §3: (a) the **primary screen reflows on resize** — `Term::resize` hardcodes `self.grid.resize(!is_alt, ..)`, no config knob, so "reflow-on-resize is suppressed" is unachievable without patching alacritty; the alt screen (tmux, Grove's actual regime) agrees with vt100. (b) alacritty retains an `ED 2`-cleared screen in scrollback where vt100 drops it. Both pinned by `crates/grove-terminal/tests/divergence.rs`. |
| 03 | App shell (entities/globals, theme, fonts, zoom, keymap skeleton, AnimationClock, storage) | Shell opens, themed, metric assertion passes | done — startup metric assertion passes at **cell_w = 7.5000005** (`GROVE_GPUI_SELFTEST=1`, within the 0.001 epsilon of 7.5); 39 unit tests + clippy clean on rustc 1.95.0. **Toolchain split:** `crates/grove-gpui` is a workspace member but excluded from `default-members`, so bare `cargo build`/`cargo test` stays on 1.94.1 for the iced app and the core crates; grove-gpui is built explicitly with `cargo +1.95.0 -p grove-gpui`. CI's four `--workspace` invocations were rewritten to `-p grove -p grove-core -p grove-terminal` (grove-gpui is CI-excluded until Plan 10). **The gpui-component durable-pin decision is deferred to Plan 08** — this phase has no text inputs, so no `gpui-component` dependency and no `[patch]` section exist in the main workspace. **Carry-forward for Plan 10:** grove-gpui's clippy runs with `--no-deps`, because clippy 1.95 raises 9 new lints (`map_unwrap_or`, `duration_suboptimal_units`, …) in `crates/grove-core`, which is off-limits this phase — they must be cleaned up when the whole product moves to 1.95. |
| 04 | TerminalElement + single-session workspace + full input path | Spec §Terminal checklist rows green; keyboard byte-table test green | done (pending human sign-off on the Appendix A **Terminal** rows, which are listed in plan 04 Task 6 Step 3) — **keyboard byte-table test green** (17 cases in `terminal::keys`, incl. the parity rows pinning plain `\x1b[A..D` for modified arrows and DECCKM affecting `arrow_moves` only); 105 unit tests + clippy clean on rustc 1.95.0, iced app and grove-core untouched and green on 1.94.1. **Inverse swap:** `GroveTerm::snapshot()` does **not** pre-apply it — `Cell` carries `inverse: bool` raw (`cell.rs:20`), so `terminal::colors::resolve_pair` owns the pipeline's single swap. **CJK `force_width`:** exists at the pinned rev (`gpui/src/text_system.rs:397`) and is wired behind `forced_width`, driven by a UAX #11 wide-range table; per-run anchoring remains the primary mitigation. Appendix A row 2 decides whether the helper survives. **Pinch-zoom:** the pinned rev's `ScrollDelta` has only `Pixels`/`Lines` and exposes **no distinguishable pinch/magnify event**, so zoom is modifier+wheel only. **Amendment to Constraint 3:** `crates/grove-terminal/src/pty.rs` gained one method, `PtyHandle::take_receiver()`, because the plan's zero-wakeup blocking reader is not expressible while the handle only lends a `!Sync` `&Receiver`; nothing else in grove-terminal changed and its golden suite stays green. `arboard` and `base64` were hoisted to `[workspace.dependencies]` (same versions/features) so both front ends share one copy. |
| 05 | Sidebar tree + WorkspaceState sync | Sidebar checklist rows green | pending |
| 06 | Appbar/statusbar/attention/dock/toasts + 480ms activity task | Attention checklist rows green on both platforms | pending |
| 07 | Grid/zen/terminal tab/worktree panel | Grid+zen checklist rows green | pending |
| 08 | Modals (simple → text-heavy) | All modal checklist rows green; keyboard matrix green | pending |
| 09 | Upgrade flow, telemetry, quit paths, persistence | System checklist rows green | pending |
| 10 | Parity passes (screenshot sweep, idle-power) + delete vt100/iced | Full Appendix A signed off; iced gone; `./install.sh` green | pending |

**Standing rules for every phase plan:**
- gpui + gpui-component pinned to the git revs decided in Plan 01; never bump mid-phase.
- vt100 stays in-tree as the golden-test oracle until Plan 10.
- Behavior questions are answered by reading the iced code (file refs in spec Appendix A), never by guessing.
- rustfmt only touched files with `--edition 2021`; clippy `unwrap_used`/`expect_used` deny applies.
- Run `./install.sh` at the end of each phase.
- Toolchain: rustc 1.95.0 via user-local rustup (`spikes/rust-toolchain.toml`); gpui_platform bootstrap + [patch] pin per findings doc.
