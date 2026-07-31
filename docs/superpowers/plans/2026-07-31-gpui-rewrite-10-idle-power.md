# Plan 10 Task 2 — idle-power measurement, gpui vs iced

**Script:** `scripts/idle-power.sh` (re-runnable; the user re-runs it at the Phase B gate)
**Row P5 of the sign-off** reads this document and accepts or rejects each scenario.

## Method (verbatim, one paragraph)

Spike S1's method, unchanged: read `utime` (field 14) + `stime` (field 15) from
`/proc/<pid>/stat` at t=0 and again at t=60 s, divide the delta by
`getconf CLK_TCK` (100 on this box), and report
`%CPU = 100 * ticks_delta / (CLK_TCK * 60)`. `pidstat` is not installed on this
box; the /proc delta is the same measurement without the dependency. `top`,
`powertop` and RAPL are **not** substituted — a different method would make
these numbers incomparable to the spike's, which is the whole point. Every
sample is taken with the window **open, sized 1280×800, and unfocused**; the
script aborts the sample if the pid dies or if the process's toplevel-window
count changes mid-sample.

## Machine / session

| | |
|---|---|
| session type | **Wayland** (`XDG_SESSION_TYPE=wayland`, compositor **Hyprland**, `WAYLAND_DISPLAY=wayland-1`) |
| X11 | **untested** — every number below is Wayland-only. The X11 half is an operator task at the gate (`WAYLAND_DISPLAY= …`). |
| `getconf CLK_TCK` | 100 |
| gpui build | `PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 build --release -p grove-gpui` → `target/release/grove-gpui` |
| iced build | `~/.local/bin/grove` (release, installed) |
| window geometry | pinned to exactly 1280×800 via `hyprctl dispatch resizewindowpixel exact 1280 800,pid:<pid>`, verified through `hyprctl -j clients` |
| theme | default (TokyoNight dark) in both |
| config isolation | each instance ran under its own `GROVE_CONFIG_DIR` so neither could touch the user's live Grove (pid 1391610, left running and untouched throughout) |

## Exact commands run

```bash
SCRATCH=<scratchpad>
rm -rf $SCRATCH/cfg-gpui $SCRATCH/cfg-iced && mkdir -p $SCRATCH/cfg-gpui $SCRATCH/cfg-iced

GROVE_CONFIG_DIR=$SCRATCH/cfg-gpui setsid nohup ./target/release/grove-gpui &
hyprctl dispatch setfloating "pid:$P"
hyprctl dispatch resizewindowpixel "exact 1280 800,pid:$P"
hyprctl dispatch focuswindow "pid:<some other window>"     # leave it UNFOCUSED
./scripts/idle-power.sh --pid $P --windows 3 --window-secs 60 --label A-gpui-fresh

GROVE_CONFIG_DIR=$SCRATCH/cfg-iced setsid nohup ~/.local/bin/grove &
# …same pin/unfocus…
./scripts/idle-power.sh --pid $P --windows 3 --window-secs 60 --label A-iced-fresh-retry
```

## Raw tick deltas

The raw deltas are the evidence; the percentage is the summary.

### Scenario A — one idle session, no PTY output, unfocused

**The matched pair** (both from a freshly created empty `GROVE_CONFIG_DIR`,
same protocol, same geometry, taken minutes apart on an otherwise quiet
desktop):

| build | w1 ticks | w2 ticks | w3 ticks | total / 180 s | mean %CPU |
|---|---|---|---|---|---|
| **grove-gpui** (`A-gpui-fresh`) | 149→245 = **96** | 246→347 = **101** | 347→446 = **99** | 296 | **1.64 %** |
| **iced** (`A-iced-fresh-retry`) | 64→188 = **124** | 188→346 = **158** | 346→506 = **160** | 442 | **2.46 %** |

Earlier, non-matched runs, recorded because they are part of the raw record and
because their spread is itself a caveat:

| run | windows (%CPU) | mean | note |
|---|---|---|---|
| `A-gpui-idle-unfocused` (first launch, config dir empty, before the home terminal had spawned) | 0.12, 0.12, 2.70 | 0.98 % | the 2.70 window coincides with the process exiting; treat it as void |
| `A-iced-idle-unfocused` (same protocol) | 0.55, 0.52, 0.47 | 0.51 % | this instance had **no** home-terminal child at all |
| `A-gpui-idle-unfocused-rerun` (config dir carrying **3** prior home terminals, 2 of them duplicate attaches to one tmux target) | 1.78, 1.73, 1.85, 6.52, 6.58 | 3.69 % | not Scenario A — extra shells; see the confounder below |
| `A-iced-fresh` (aborted) | 2.67, then the pid vanished mid-window-2 | — | sample void by the script's own guard |

Spike S1's published baseline, for orientation:

| build | %CPU over 60 s (spike S1) |
|---|---|
| spike-term, release, blink on | 1.23 % |
| spike-term, release, `SPIKE_NO_BLINK=1` | 0.00 % |
| real Grove (`~/.local/bin/grove`, release) | 3.85 % / 3.55 % |

### Scenarios B, C, D — **NOT MEASURED**

| scenario | gpui | iced | status |
|---|---|---|---|
| A. one idle session, no PTY output, unfocused | **1.64 %** | **2.46 %** | measured |
| B. same + one background agent actively streaming output | — | — | **NOT MEASURED** |
| C. grid view, n=5, all idle, unfocused | — | — | **NOT MEASURED** |
| D. zen, single session, idle, unfocused | — | — | **NOT MEASURED** |

**Why:** B, C and D each require driving the GUI — spawning an agent that
streams, creating five sessions and entering grid view, toggling zen. Neither
build has a scripted UI driver, and synthesising input into the user's live
Wayland session was judged too invasive to do unattended. These three are
therefore **operator tasks at the gate**, not agent findings. `idle-power.sh`
is written to make each a two-command job:

```bash
# put the window in the state, leave it unfocused, then:
./scripts/idle-power.sh --pid $(pgrep -f target/release/grove-gpui) --windows 3 --label B-gpui
./scripts/idle-power.sh --pid $(pgrep -f '\.local/bin/grove')      --windows 3 --label B-iced
```

## Verdicts

| scenario | verdict |
|---|---|
| **A** | **gpui ≤ iced** — 1.64 % vs 2.46 % on the matched pair. Both are below spike S1's 3.55–3.85 % baseline for the installed iced build. The spec §4 adaptive-tick claim is **not** contradicted by this measurement. |
| **B** | **inconclusive — not measured.** |
| **C** | **inconclusive — not measured.** |
| **D** | **inconclusive — not measured.** |

## Caveats that must be read before accepting scenario A

These are stated plainly rather than editorialised away; a rejection here is a
legitimate reason to refuse Phase C.

1. **The two builds do not spawn the same home terminal.** With an identical
   empty config, grove-gpui spawned **two `tmux -L grove … attach-session`
   children pointed at the same target** (`…__terminal__6` twice), while the
   iced build spawned a **single native `/usr/bin/zsh`**. So Scenario A is
   comparing "gpui + 2 tmux clients" against "iced + 1 native shell". The
   duplicate attach is reproducible and looks like a real defect, but it is
   **not on any checklist** and was therefore not fixed here — Task 1 Step 4's
   rule is that a new functional gap is a scope decision, not a worker
   decision. It is carried into the sign-off's open decisions.
2. **Run-to-run spread is large.** The same gpui binary measured 0.12 %,
   1.64 % and 3.69 % across three sessions that differed only in how many home
   terminals the config carried. Any single number here is weak evidence; the
   matched pair is the only comparison in this document that controls for it.
3. **Both processes exited spontaneously** during two of the runs (silently,
   with empty logs), voiding those samples. The script's pid guard caught each
   one. The cause was not diagnosed and may be an artefact of this unattended
   environment rather than either build.
4. **Wayland only.** No X11 number exists for either build.
5. **Three windows, not more.** 3×60 s per build is what the spike used; it is
   not enough to characterise a tail.
