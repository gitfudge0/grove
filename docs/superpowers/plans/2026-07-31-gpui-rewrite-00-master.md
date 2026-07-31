# gpui Rewrite — Master Plan Index

> **For agentic workers:** This is an index, not an executable plan. Execute the numbered phase plans in order; each is a standalone plan document following superpowers:writing-plans. Later plans are written just-in-time when their phase starts, incorporating findings from earlier phases.

**Spec:** `docs/superpowers/specs/2026-07-31-gpui-rewrite-design.md` (the authority for all behavior; Appendix A is the acceptance checklist)

**Branch:** all work happens on `gpui-rewrite` (big-bang branch off `main`).

| # | Plan | Exit gate | Status |
|---|------|-----------|--------|
| 01 | Spikes (terminal element, text inputs, zoom, Linux matrix) | Findings doc committed; go/no-go + rev pins decided | done — findings committed; awaiting user go/no-go |
| 02 | `crates/grove-terminal` + dual-parser golden harness | Golden tests green against vt100 oracle | pending |
| 03 | App shell (entities/globals, theme, fonts, zoom, keymap skeleton, AnimationClock, storage) | Shell opens, themed, metric assertion passes | pending |
| 04 | TerminalElement + single-session workspace + full input path | Spec §Terminal checklist rows green; keyboard byte-table test green | pending |
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
