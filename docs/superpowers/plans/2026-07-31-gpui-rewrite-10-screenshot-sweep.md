# Plan 10 Task 3 — the parity screenshot sweep (index)

**Script:** `scripts/screenshot-sweep.sh` — `./scripts/screenshot-sweep.sh {gpui|iced}`
**Shots:** `target/parity-shots/{gpui,iced}/<slug>.png` — **gitignored, never committed.** They are review material, not source.
**Row P6 of the sign-off** is "work down this index and fill every verdict cell".

> ## **EVERY VERDICT CELL STARTS EMPTY.**
>
> An agent filling one in is the same violation as an agent signing a checklist
> row. The bar is the spec's own (§1): **"pixel-familiar, not pixel-perfect"** —
> a different anti-aliasing is a **pass**; a missing element or a wrong layout is
> a **fail**.

## Capture mechanism

Probed on this box before the script was written:

| tool | present |
|---|---|
| `grim` | yes |
| `slurp` | yes |
| `hyprctl` | yes |
| `jq` | yes |
| `magick` (ImageMagick 7) / `import` | yes |
| `ydotool` | yes (not used — see limitations) |
| `xwd`, `spectacle`, `gnome-screenshot` | **absent** |

Session: **Wayland**, compositor **Hyprland** (`XDG_SESSION_TYPE=wayland`,
`XDG_CURRENT_DESKTOP=Hyprland`).

**Chosen mechanism:** `hyprctl -j clients` resolves the target window's exact
`at`/`size`, and `grim -g "X,Y WxH"` captures precisely that rectangle. No
manual region selection, so a *pair* is pixel-aligned by construction rather
than by a steady hand. On X11 the fallback would be `import -window <id>`; it is
wired conceptually but **untested here**.

**Geometry pinning** — a pair is only comparable if both builds are the same
size. The script pins the window with

```bash
hyprctl dispatch setfloating "pid:$APP_PID"
hyprctl dispatch resizewindowpixel "exact 1280 800,pid:$APP_PID"
```

re-checks the geometry before **every** capture, and refuses to shoot if it
drifted. On a non-Hyprland compositor, size the window by hand once and pass
`--no-pin`.

## Base configuration

**1280x800, zoom 1.0, TokyoNight dark, chrome visible.** Every slug in the
*Screens* and *Modals* sections is captured at exactly that; the cross-section
slugs vary exactly one axis each.

Both builds are release:

```bash
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 build --release -p grove-gpui
./scripts/screenshot-sweep.sh gpui     # runs target/release/grove-gpui
./scripts/screenshot-sweep.sh iced     # runs ~/.local/bin/grove
```

## Stratification — why this list and not spec 8.3 literally

Spec 8.3 asks for *every screen/modal x 3 zooms x 4 representative themes +
follow-system flip x grid n in {1,2,3,5} x panel open/zen*. Taken literally that
is a combinatorial explosion nobody will review. The sweep is therefore
**stratified**: the full screen/modal list once at the base configuration, plus
a zoom/theme cross-section over a representative subset. **92 slugs per build,
184 shots, 92 pairs.**

## The four themes

Named here, as the plan requires:

| axis | theme | note |
|---|---|---|
| dark | **tokyonight** (storm) | the default; what the base configuration uses |
| light | **tokyonight-day** | its light counterpart |
| high-contrast | **_operator names the highest-contrast built-in here_** | must be filled before the sweep, or the pair is unlabelled |
| custom | **_operator names a theme from their own `themes.json` here_** | same |

The last two are deliberately left blank: this box's `themes.json` is the
user's, and naming a theme they do not have would make the row unreviewable.

## Sweep status — what was actually captured

| | |
|---|---|
| slugs enumerated | **92 per build** |
| pairs captured by the agent | **1** (`workspace-empty-no-projects`) |
| pairs outstanding | **91** |

**Why only one.** The sweep is by design *operator-driven*: gpui at ZED_REV
exposes no screenshot API and neither build has a scripted UI driver, so 91 of
the 92 slugs need a human to reach the state (open a modal, spawn five sessions,
drag a tile, force a spawn failure). `ydotool` exists on this box, but
synthesising input into the user's live Wayland session unattended was judged
too invasive and too fragile to trust as evidence. `workspace-empty-no-projects`
is the one state reachable with no interaction at all — launch against a fresh
`GROVE_CONFIG_DIR` and shoot — so it was captured for **both** builds to prove
the mechanism end to end. It works.

### Known limitation found while capturing

`grim` cannot capture pixels beyond the output's edge. When the pinned 1280x800
window sat so its right edge ran past the monitor boundary, the captured PNG
carried a blank band there. **Move the window fully on-screen before starting
the sweep** — the script pins the *size*, not the *position*.

## The pairs

Paths are relative to the repo root. An em-dash means the shot has not been
taken yet.

| # | slug | gpui | iced | reviewed | verdict |
|---|---|---|---|---|---|
| 1 | `workspace-empty-no-projects` | `target/parity-shots/gpui/workspace-empty-no-projects.png` | `target/parity-shots/iced/workspace-empty-no-projects.png` | [ ] | |
| 2 | `workspace-empty-has-projects` | — | — | [ ] | |
| 3 | `workspace-single-session` | — | — | [ ] | |
| 4 | `sidebar-collapsed` | — | — | [ ] | |
| 5 | `sidebar-sessions-only` | — | — | [ ] | |
| 6 | `sidebar-all` | — | — | [ ] | |
| 7 | `sidebar-hover-actions` | — | — | [ ] | |
| 8 | `sidebar-agent-menu` | — | — | [ ] | |
| 9 | `sidebar-git-suffix` | — | — | [ ] | |
| 10 | `sidebar-archived-empty-state` | — | — | [ ] | |
| 11 | `sidebar-terminals-expanded` | — | — | [ ] | |
| 12 | `sidebar-terminals-docked` | — | — | [ ] | |
| 13 | `grid-n1` | — | — | [ ] | |
| 14 | `grid-n2` | — | — | [ ] | |
| 15 | `grid-n3` | — | — | [ ] | |
| 16 | `grid-n5` | — | — | [ ] | |
| 17 | `grid-drag-target` | — | — | [ ] | |
| 18 | `grid-tile-waiting` | — | — | [ ] | |
| 19 | `zen-single` | — | — | [ ] | |
| 20 | `zen-attention-pill` | — | — | [ ] | |
| 21 | `terminal-tab` | — | — | [ ] | |
| 22 | `terminal-tab-multiple` | — | — | [ ] | |
| 23 | `panel-20` | — | — | [ ] | |
| 24 | `panel-40` | — | — | [ ] | |
| 25 | `panel-75` | — | — | [ ] | |
| 26 | `panel-tabs-multi-shell` | — | — | [ ] | |
| 27 | `appbar-attention-pill` | — | — | [ ] | |
| 28 | `attention-dropdown` | — | — | [ ] | |
| 29 | `statusbar-default` | — | — | [ ] | |
| 30 | `statusbar-toast-info` | — | — | [ ] | |
| 31 | `statusbar-toast-error` | — | — | [ ] | |
| 32 | `session-header-working` | — | — | [ ] | |
| 33 | `modal-input` | — | — | [ ] | |
| 34 | `modal-confirm` | — | — | [ ] | |
| 35 | `modal-confirm-quit` | — | — | [ ] | |
| 36 | `modal-addproject-step1` | — | — | [ ] | |
| 37 | `modal-addproject-autocomplete` | — | — | [ ] | |
| 38 | `modal-addproject-step2-git-init` | — | — | [ ] | |
| 39 | `modal-removeproject` | — | — | [ ] | |
| 40 | `modal-removeproject-progress` | — | — | [ ] | |
| 41 | `modal-archiveproject` | — | — | [ ] | |
| 42 | `modal-archived-list` | — | — | [ ] | |
| 43 | `modal-message` | — | — | [ ] | |
| 44 | `modal-tmuxchoice` | — | — | [ ] | |
| 45 | `modal-agentpicker` | — | — | [ ] | |
| 46 | `modal-launcher-recents` | — | — | [ ] | |
| 47 | `modal-launcher-filtered` | — | — | [ ] | |
| 48 | `modal-launcher-row-actions` | — | — | [ ] | |
| 49 | `modal-launcher-switch-drill` | — | — | [ ] | |
| 50 | `modal-launcher-settings-drill` | — | — | [ ] | |
| 51 | `modal-launcher-theme-preview` | — | — | [ ] | |
| 52 | `modal-themepicker-dark` | — | — | [ ] | |
| 53 | `modal-themepicker-light` | — | — | [ ] | |
| 54 | `modal-themepicker-project-scope` | — | — | [ ] | |
| 55 | `modal-thememanager` | — | — | [ ] | |
| 56 | `modal-theme-editor` | — | — | [ ] | |
| 57 | `modal-settings-general` | — | — | [ ] | |
| 58 | `modal-settings-tools` | — | — | [ ] | |
| 59 | `modal-shortcutoverlay-workspace` | — | — | [ ] | |
| 60 | `modal-shortcutoverlay-grid` | — | — | [ ] | |
| 61 | `modal-teardown-running` | — | — | [ ] | |
| 62 | `modal-teardown-done` | — | — | [ ] | |
| 63 | `modal-scriptseditor` | — | — | [ ] | |
| 64 | `modal-updating` | — | — | [ ] | |
| 65 | `modal-changelog` | — | — | [ ] | |
| 66 | `onboarding-step1` | — | — | [ ] | |
| 67 | `onboarding-step2` | — | — | [ ] | |
| 68 | `zoom060-workspace-single-session` | — | — | [ ] | |
| 69 | `zoom060-grid-n3` | — | — | [ ] | |
| 70 | `zoom060-panel-40` | — | — | [ ] | |
| 71 | `zoom060-modal-launcher-recents` | — | — | [ ] | |
| 72 | `zoom060-modal-settings-general` | — | — | [ ] | |
| 73 | `zoom200-workspace-single-session` | — | — | [ ] | |
| 74 | `zoom200-grid-n3` | — | — | [ ] | |
| 75 | `zoom200-panel-40` | — | — | [ ] | |
| 76 | `zoom200-modal-launcher-recents` | — | — | [ ] | |
| 77 | `zoom200-modal-settings-general` | — | — | [ ] | |
| 78 | `theme-dark-workspace-single-session` | — | — | [ ] | |
| 79 | `theme-dark-grid-n3` | — | — | [ ] | |
| 80 | `theme-dark-modal-themepicker-dark` | — | — | [ ] | |
| 81 | `theme-light-workspace-single-session` | — | — | [ ] | |
| 82 | `theme-light-grid-n3` | — | — | [ ] | |
| 83 | `theme-light-modal-themepicker-dark` | — | — | [ ] | |
| 84 | `theme-contrast-workspace-single-session` | — | — | [ ] | |
| 85 | `theme-contrast-grid-n3` | — | — | [ ] | |
| 86 | `theme-contrast-modal-themepicker-dark` | — | — | [ ] | |
| 87 | `theme-custom-workspace-single-session` | — | — | [ ] | |
| 88 | `theme-custom-grid-n3` | — | — | [ ] | |
| 89 | `theme-custom-modal-themepicker-dark` | — | — | [ ] | |
| 90 | `follow-system-first-frame-dark` | — | — | [ ] | |
| 91 | `follow-system-first-frame-light` | — | — | [ ] | |
| 92 | `grid-n3-panel-open` | — | — | [ ] | |

## Recorded N/A

| slug | why |
|---|---|
| `grid-n3-panel-open` | **N/A, not a miss.** Plan 07 recorded that the worktree panel is *suppressed in grid view, with no exception*, so the combination is unreachable by design in both builds. |

## How to record a one-sided slug

If a slug is reachable in one build and not the other, capture the one that
works and write **MISSING in \<build\>** in its verdict cell. That is a
**finding**, not a skip — a state that exists in iced and not in gpui is exactly
what this sweep is for.
