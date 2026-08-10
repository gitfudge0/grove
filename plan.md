# Modal standardization plan (UMModal)

Source: design review in `ummock.html` (full audit + proposed standard, frames A1–A17 / B1–B8)
and `ummock-variants.html` (option board; winners chipped ✓ CHOSEN).
Decisions made 2026-08-10.

## Decisions

### 1. Input fields — variant C1c "boxed + focus ring"

Replaces the bare underline (`field_underline`).

- Box: full width, `px 10 / py 7` (~32px tall), radius `RADIUS_GROUP` (6),
  fill `BG_STRIP()`, 1px `BORDER_SOFT()`.
- Focused: 1px `MAGENTA()` border + 2px magenta-tinted outer ring
  (~25% alpha; theme-aware, needs its own derived color in `theme.rs`).
- Mono 12px text, same zeroed-inset `Input` contract as today.
- **Sanctioned exception — launcher search zone**: headerless, borderless
  input directly on the rail (`BG_RAIL()`), no box/ring; the divider below
  the zone is the only separator. Command-palette surfaces only.

### 2. Footers — variant C2g "statusbar"

Replaces the `BG_STRIP()` footer strip.

- No strip fill: 1px `BORDER_SOFT()` top divider + transparent `py 8` row
  on the rail. Bottom panel radius returns to full `RADIUS_PANEL` (12) —
  the 11px inner value (`FOOTER_RADIUS`) retires.
- Hints **left-aligned**, mono 10px, transparent keycaps.
- Buttons right, gap 8, always secondary(Plain) → affirmative(Primary/Danger).
- Hints-only modals (launcher, shortcuts, archived projects) = same row
  minus the button group. One vocabulary everywhere.
- **Left footer slot retires.** Its content relocates:
  - AgentPicker "Default" → flat cyan text action at the foot of the body.
  - ThemeManager "+ New theme" → flat text action in body (was a Primary
    button in the left slot — contract violation anyway).
  - Settings version string / update status / restart → header meta or body.
  - ScriptsEditor "Archive project" → flat tinted text action in body.

### 3. Accompanying rules (from the B2 rules card)

- Width by content class: SM 420 sentence-only · MD 480 form/short list ·
  LG 560 rows with secondary column · XL 640 scrolling list/palette.
  Per this rule: AgentPicker 560→480, ThemePicker 480→560.
- One header component with optional slots (meta/step counter, subtitle).
  Close X always present except blocking-progress states
  (RemoveProject in-progress, Teardown running, Updating).
- One overflow strategy: body `max_h MODAL_SCROLL_MAX_H (456)` + scroll.
  Retire the 6-row window (AddProject dir list), the 8-row window
  (ThemePicker), and cap the uncapped ShortcutOverlay body.
- `esc` hint vocabulary: exactly `cancel` (abandons input) / `close`
  (nothing to lose) / `back` (returns to parent).
- Dismiss-only modals: single `Close` button, always Primary
  (fixes Message vs UpdateFailed vs Changelog inconsistency).
- Fix Updating(Updated) button order: `Later` (Plain) then `Restart`
  (Primary) — currently the app's only primary-first footer.
- No bordered buttons inside bodies ("Change", "Kill all sessions",
  onboarding "Browse…") — in-body actions become flat tinted text.
- One boxed-group recipe: `card()` (radius 4, `BORDER`, `BG_STRIP`).
  Radius rule: 4 controls/cards · 6 rows/groups/fields · 12 panel.
  `SWATCH_RADIUS 2` moves onto the scale or gets tokenized.
- One focus/selection language: magenta = keyboard focus, cyan tint +
  `SEL_RING` = selection. The yellow agent-bar ring retires.
- Panel shadow becomes a theme token; light theme gets a lighter,
  tighter shadow (today: hard-coded `rgba(0,0,0,.35) 0 12px 40px`).

## Implementation punch list (not yet applied — code untouched)

1. `src/theme.rs`: add focus-ring color + shadow tokens (light/dark).
2. `src/views/tokens.rs`: field height/padding tokens; retire `FOOTER_RADIUS`;
   width reassignments per class.
3. `src/views/components.rs`: new `field_box()` replacing `field_underline()`
   (keep the zeroed-inset Input contract); rework `footer_container` /
   `modal_footer` to the statusbar scheme (hints left, no strip, no left
   slot); flat in-body action primitive.
4. Migrate call sites: all 8 files in `src/views/modals/` + onboarding.
   Collapse the three header forks (wizard, Settings, ScriptsEditor) into
   the one slotted header.
5. Overflow: apply the 456 scroll cap to AddProject dir list, ThemePicker
   list, ShortcutOverlay.
6. Hint copy sweep to the cancel/close/back vocabulary.
7. Update conformance suite (`src/views/conformance.rs`, rules R1–R8) to
   enforce the new contracts; `cargo test`.
8. `./install.sh` after changes (per CLAUDE.md).

## Known risks / review notes

- C2g hints are mono 10px `FG_MUTE` on the bare rail — lowest-contrast
  text in the design; verify legibility in light themes at 100% zoom.
- The focus ring's tinted alpha can't be derived from `MAGENTA()` alone;
  needs a proper `theme.rs` derivation across all ~30 bundled themes.
- Launcher's `Input` is currently the only non-inset-zeroed field; fix it
  while touching the search zone.
