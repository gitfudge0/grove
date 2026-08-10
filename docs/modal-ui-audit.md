# Modal UI consistency audit

Every modal in the app, its rendered UI, and every inconsistency between them.

Source of truth: `src/modal.rs` (`ModalKind::ALL`, 19 kinds), `src/views/modals/*`,
`src/views/components.rs` (shared chrome), `src/views/tokens.rs` (scales).
Read at commit `HEAD` on 2026-08-09.

19 kinds render as **23 distinct panels** — several kinds swap their whole panel
per sub-state (ThemeManager has three, Teardown three, RemoveProject two).

---

## 1. The inventory

Width tokens: `SM` 420 · `MD` 480 · `LG` 560 · `XL` 640 (`tokens.rs:53-62`).

| # | Modal (state) | Source | Width | Header | Close X | Divider after header | Body zone | Actions live in | Footer hints |
|---|---|---|---|---|---|---|---|---|---|
| 1 | Input | `confirm.rs:284` | MD | `modal_header`, MAGENTA | — | ✅ | hand-rolled zones | body | `⏎ confirm` `esc cancel` |
| 2 | Confirm (safe) | `confirm.rs:364` | MD | `modal_header`, MAGENTA | — | — | `modal_body`, gap 2XL | body | `⏎ confirm` `esc cancel` |
| 3 | Confirm (destructive) | `confirm.rs:364` | MD | `modal_header`, RED | — | — | `modal_body`, gap 2XL | body | `y remove` `esc cancel` |
| 4 | Message | `confirm.rs:424` | MD | "Notice", CYAN | — | — | `modal_body`, gap 2XL | body | `esc close` |
| 5 | TmuxChoice | `confirm.rs:456` | MD | "Session backend", CYAN | — | — | `modal_body`, gap XL | body | `⏎ tmux` `n native` `esc close` |
| 6 | AgentPicker | `confirm.rs:505` | **LG** | `modal_header`, MAGENTA | — | — | `modal_body`, gap 2XL | body | `↑↓ choose` `⏎ launch` `esc cancel` |
| 7 | RemoveProject (confirm) | `project.rs:544` | LG | "Remove project", RED | — | — | `modal_body`, gap XL | body | `y remove` `space toggle delete` `esc cancel` |
| 8 | RemoveProject (progress) | `project.rs:557` | LG | same | — | — | `modal_body`, gap MD | — | **none** |
| 9 | ArchiveProject | `project.rs:684` | **SM** | `Archive '{n}'?`, AMBER | — | — | `modal_body`, gap XL | **footer** | `y archive` `n cancel` |
| 10 | ArchivedProjects | `project.rs:847` | LG | `modal_header_with_close`, MAGENTA | ✅ 22×22 / 12px | — | `modal_body` + scroll 360 | — | `esc close` |
| 11 | Teardown (running) | `project.rs:943` | LG | `Delete worktree / {n}`, RED | — | — | `modal_body`, gap XL + PTY 240 | body | `esc skip & remove` |
| 12 | Teardown (removing) | `project.rs:943` | LG | same | — | — | same | — | **none** |
| 13 | Teardown (done) | `project.rs:943` | LG | same | — | — | same | body | `esc close` |
| 14 | AddProject step 1 | `add_project.rs:585` | **XL** | `wizard_header(1)` + step counter | — | — | `modal_body`, gap 2XL | body | `tab complete` `↑↓ select` `⏎ continue` `esc cancel` |
| 15 | AddProject step 2 | `add_project.rs:680` | XL | `wizard_header(2)` + step counter | — | — | `modal_body`, gap 2XL | body | `⏎ add` `esc back` |
| 16 | Onboarding ×4 steps | `add_project.rs:882` | LG *(content col, no panel)* | own brand + progress rail | — | n/a | own layout | own footer row | **none** (own footer) |
| 17 | SessionLauncher ×4 views | `launcher.rs:717` | **760 (off-scale)** | **none** — search row | — | ✅ | scroll 452 | — | varies per view |
| 18 | ThemePicker | `theme_picker.rs:556` | MD | `modal_header`, MAGENTA | — | — | `modal_body`, gap XL | body | **none** |
| 19 | ThemeManager (list) | `theme_picker.rs:723` | LG | `modal_header_with_close`, MAGENTA | ✅ 22×22 / 12px | ✅ | `modal_body` + scroll 360 | body | `↑↓ select` `esc close` |
| 20 | ThemeManager (editor) | `theme_picker.rs:752` | **XL** | "Theme editor", MAGENTA | — | — | `modal_body`, gap LG | body | `tab indent` `esc back` |
| 21 | ThemeManager (delete) | `theme_picker.rs:811` | **SM** | "Delete theme", RED | — | — | `modal_body`, gap LG | body | `y delete` `esc cancel` |
| 22 | Settings | `settings.rs:767` | XL | `modal_header_row`, MAGENTA | ✅ **28×22 / 14px** | ✅ | `modal_body` + scroll 456 | **footer** | `esc close` (in custom footer) |
| 23 | ShortcutOverlay | `settings.rs:1274` | XL | "Keyboard shortcuts", MAGENTA | — | — | `modal_body` | — | `esc close (or press the same chord again)` |
| 24 | ScriptsEditor | `settings.rs:1400` | LG | `modal_header_row` + mono subtitle | ✅ **28×22 / 14px** | ✅ | `modal_body` + scroll 456 | **footer** | `esc discard` (in custom footer) |
| 25 | Updating ×3 states | `settings.rs:1711` | SM | "Updating Grove", MAGENTA | — | — | `modal_body` | body | none / `esc later` / `esc close` |
| 26 | Changelog | `settings.rs:1792` | LG | "Changelog", MAGENTA | — | — | `modal_body`, gap LG + scroll 420 | body | `esc back to settings` |

Two documented layout exceptions (`modals/mod.rs:8-12`): **Onboarding** replaces the
screen (no scrim, no panel); **SessionLauncher** top-drops 80px instead of centering.
Both are intentional and not counted as drift below.

---

## 2. Inconsistencies

### A. Panel width

**A1. The palette is off the width scale.** `PALETTE_W = 760` (`launcher.rs:41`)
while `tokens.rs:62` says of `MODAL_W_XL` (640): *"Nothing goes above this."* The
constant even documents itself as "wider than `MODAL_W_XL` (shared by every other
modal)" — a rule and its exception written 600 lines apart.

**A2. One modal, three widths.** ThemeManager renders at LG (list), XL (editor,
`:752`) and SM (delete confirm, `:811`). Drilling into a theme grows the panel by
80px, then deleting one shrinks it by 220px. Teardown and RemoveProject hold their
width across stages; ThemeManager does not.

**A3. ArchiveProject is SM but is not an SM modal.** `MODAL_W_SM` is documented as
"confirmations and single-question modals — one short paragraph, no list"
(`tokens.rs:52`). ArchiveProject (`project.rs:809`) hosts a per-session list, a
bordered strip, a "Kill all sessions (N)" button, a caption *and* two footer
buttons at 420px.

**A4. Equivalent modals, different widths.** AgentPicker (a list of choices + launch/cancel)
is LG; ThemePicker (a list of choices + apply/cancel) is MD.

### B. Header

**B5. Two close-button geometries.** `modal_header_with_close` uses
`icon_btn(CONTROL_H=22 box, ICON_SM=12 glyph)` (`components.rs:218`); Settings
(`settings.rs:781`) and ScriptsEditor (`settings.rs:1513`) use
`flat_icon_btn(ICON_BTN_W=28 box, ICON_MD=14 glyph)`. The same "close X" is two
sizes depending on which modal you're in.

**B6. Close button presence is arbitrary.** Only 4 of 23 panels have one
(ArchivedProjects, ThemeManager-list, Settings, ScriptsEditor). ShortcutOverlay,
Changelog and the ThemeManager **editor** are all long, scrolling, non-destructive
panels with no visible dismiss — and the editor is a sub-view of a modal that *does*
have one, so the X disappears when you drill in.

**B7. Header accent has no rule.** MAGENTA (13 panels) is the default; RED marks
destructive (4); **CYAN** marks Message and TmuxChoice only (`confirm.rs:428,460`) —
and CYAN is otherwise the palette drill-in cue colour (`components.rs:1009`);
**AMBER** is used exactly once, for ArchiveProject (`project.rs:811`). Two colours
with one or two consumers each and no stated semantics.

**B8. Comment/code drift on the header shape.** `settings.rs:1434` says ScriptsEditor
has "the same shape App Settings' header now has (title + subtitle + close)". App
Settings has no subtitle (`settings.rs:775-785`). ScriptsEditor is the only panel with one.

**B9. Progress indicator placement.** AddProject puts "Step 1 of 2" in the header
(`add_project.rs:576`); Onboarding puts the identical "1 / 4" count in its footer
(`add_project.rs:1161`).

### C. Zone structure

**C10. The header divider is present in 5 of 23 panels.** Input (`confirm.rs:352`),
SessionLauncher (`launcher.rs:785`), ThemeManager-list (`theme_picker.rs:1029`),
Settings (`settings.rs:1124`), ScriptsEditor (`settings.rs:1700`). The other 18 run
the body straight off the header. `modal_body`'s doc (`components.rs:568-572`)
treats the divider as the thing that separates the zones — so 18 panels are missing
the rule that doc assumes.

**C11. Same for the footer divider** — only Input, SessionLauncher and
ThemeManager-list draw one above the footer strip.

**C12. Input doesn't use `modal_body`.** It hand-rolls an input zone
(`px/py SPACE_3XL`) and a button zone (`px SPACE_3XL / py SPACE_2XL`)
(`confirm.rs:290-322`), so its internal vertical rhythm matches nothing else.

**C13. Six different body gaps.** `modal_body` already sets `gap(SPACE_XL)`, then
callers nest a second column with their own: 2XL (Confirm, Message, AgentPicker,
AddProject), XL (TmuxChoice, ThemePicker, RemoveProject-confirm, Teardown), LG
(ThemeManager editor + delete, Changelog, Updating-updated), MD (RemoveProject-progress,
Updating-failed). Nothing selects between them.

### D. Text roles

**D14. `body_text()` exists and half the modals ignore it.** `components.rs:586`
defines body prose as TEXT_BODY/FG_DIM. Confirm (`confirm.rs:391`), Message (`:434`),
TmuxChoice (`:466`), RemoveProject (`project.rs:587`) and Teardown (`project.rs:969`)
all render the same "one paragraph of body prose" as `ui(..., TEXT_TITLE, FG_DIM)` —
one type tier louder. ArchiveProject, ThemeManager, Updating, Changelog and
AddProject use `body_text` correctly.

**D15. Two components for the same validation note.** `note_text()`
(TEXT_SMALL/RED, `components.rs:592`) is used by Onboarding (`add_project.rs:1048`)
and ThemeManager rename (`theme_picker.rs:907`). Input (`confirm.rs:324`) and both
AddProject steps (`add_project.rs:647, 780`) render the same inline error as
`ui(..., TEXT_BODY, RED)` — a different size for identical semantics.

**D16. Section labels forked.** `section_header()` (mono/TEXT_MICRO/FG_MUTE with an
indent contract) is used by the palette, Onboarding, Settings cards and the shortcut
overlay. AddProject step 2 open-codes its own as `mono("Folder", TEXT_SMALL, FG_MUTE)`
and `mono("Name", …)` (`add_project.rs:742, 749`) — a different tier, no indent.

**D17. Onboarding's rail labels are lowercase** ("welcome", "environment",
`modal.rs:90-94`) while every other section label in the app is uppercase.

**D18. The same agent label renders three ways.** AgentPicker: `ui(TEXT_BODY)`
(`confirm.rs:546`). Onboarding: `ui(TEXT_TITLE)` (`add_project.rs:1081`). Palette
strip: `mono(TEXT_BODY)` (`launcher.rs:1163`).

### E. Buttons

**E19. Action rows live in three places.** Body (18 panels), footer strip
(ArchiveProject `project.rs:796`, Settings `settings.rs:1088`, ScriptsEditor
`settings.rs:1662`), or nowhere-plus-no-footer (ThemePicker).

**E20. There is no disabled button.** `ModalBtn` has four weights, none disabled, so
ArchiveProject's blocked Archive button (`project.rs:762-770`) is a hand-rolled div
duplicating `modal_action_sized`'s box geometry with FG_MUTE text and a BORDER_SOFT
border. It's the only disabled control in the app and it is a fork of the component
it should be.

**E21. Two spacer idioms.** Most action rows push right with
`.child(div().flex_1())`; ThemeManager's delete confirm (`theme_picker.rs:793`) and
its "+ New theme" row (`:1016`) use `.justify_end()`.

**E22. Dismiss labels drift.** "Cancel" (most), "Close" (Message `confirm.rs:442`,
Teardown-done `project.rs:978`, Updating-failed `settings.rs:1769`), "Later"
(Updating-updated), "Back to Settings" (Changelog). "Close" vs "Cancel" for the same
plain dismiss is unmotivated; "Later"/"Back to Settings" are meaningful and fine.

**E23. Button-set shape varies.** Standard is `[spacer] Cancel · Affirmative`.
AgentPicker adds a third button ("Default") on the far left of the same row
(`confirm.rs:563`). ThemeManager's editor has **only** Save, no Cancel
(`theme_picker.rs:765`).

**E24. Same operation, two affordances.** AgentPicker sets the default agent with a
"Default" button that toggles (`confirm.rs:563`); Settings → Tools does it with a
"Set default" button that swaps to a "Default" keycap pill (`settings.rs:1168-1180`).

### F. Footer

**F25. ThemePicker has no footer strip** (`theme_picker.rs:693-698`) — the only
centred modal without one. It is therefore the only modal that never states what
Escape does, and its Escape *does* something non-obvious (returns to Settings or the
ScriptsEditor, `modal.rs:676-693`).

**F26. `esc` means seven different things across the footers:** "cancel", "close",
"back", "discard", "later", "skip & remove", "back to settings". Settings says
`esc close` and ScriptsEditor says `esc discard` for the same gesture on the same
kind of panel.

**F27. `⏎` labelling is equally loose:** "confirm", "launch", "open", "continue",
"add", "tmux". TmuxChoice's `⏎ tmux` (`confirm.rs:495`) binds Enter to the
affirmative, but the button pair renders Native and Tmux with equal visual weight
apart from Plain/Primary — the footer asserts a default the buttons barely mark.

**F28. Destructive-confirm key vocabulary splits.** Confirm-destructive,
RemoveProject and ThemeManager-delete offer `y` / `esc`; ArchiveProject offers
`y` / `n` (`project.rs:786-787`) and never mentions `esc`.

**F29. RemoveProject's footer names a control that may not exist.** It always
advertises `space toggle delete` (`project.rs:648-652`), but the checkbox is only
rendered when `worktree_count > 0` (`project.rs:597`).

**F30. Height jumps when the footer disappears.** RemoveProject-in-progress,
Teardown-Removing and Updating-Updating drop the footer strip entirely. Each is
individually documented, but `project.rs:655` explicitly chose LG over MD *to avoid a
mid-operation height change* — the same concern the vanishing footer reintroduces.

### G. Lists and rows

**G31. Four row shapes for "pick one of these".**
`palette_row` (54px, RADIUS_GROUP, SEL_TINT_SOFT + ring) — palette, AgentPicker.
`click_row(Compact)` (px LG/py SM, RADIUS_CONTROL, BG_HL) — ThemePicker, dir list,
Onboarding agents. `click_row(Manager)` — ThemeManager. A **hand-rolled div** —
ArchivedProjects (`project.rs:894-919`), which reproduces `Manager`'s px XL / py MD /
RADIUS_GROUP but hovers to `BG_HL` where `click_row` hovers to `BG_HOVER`.

**G32. Two selection languages.** `palette_row` marks selection with
SEL_TINT_SOFT + a SEL_RING border; `click_row` marks it with BG_HL and no border.
Both appear in modals that sit one Escape apart.

**G33. AgentPicker uses the 54px palette row for single-line rows**
(`confirm.rs:530`). `PALETTE_ROW_H` is sized for a title *and* a subtitle
(`components.rs:986`); the agent rows have neither, so the list is visibly airier
than ThemePicker's list of the same kind of choice.

**G34. `card()` exists and two list containers open-code it.** ThemePicker's list
container (`theme_picker.rs:641-647`) and Onboarding's agent list
(`add_project.rs:1088-1096`) both build RADIUS_CONTROL + 1px BORDER + BG_STRIP by
hand — that is exactly `components::card()` (`components.rs:670`), which Settings uses.

**G35. Four scroll caps, none shared.** `LIST_MAX_H` 360, `MODAL_SCROLL_MAX_H` 456,
`CHANGELOG_SCROLL_MAX_H` 420, `PALETTE_LIST_MAX_H` 452. Four numbers inside a 100px
band, four separate constants.

**G36. Two constants are literally duplicated.** `LIST_MAX_H = 360.0` and
`EMPTY_STATE_PY = SPACE_3XL * 2.0` are defined identically in `project.rs:43,47` and
`theme_picker.rs:53,57`.

**G37. Empty-state copy has three registers.** Sentence case with a period
("No archived projects." `project.rs:861`); sentence case with an em-dash hint
("No custom themes yet — create one or paste a palette." `theme_picker.rs:831`);
lowercase fragment, no period, left-aligned ("no matches" / "no sessions"
`launcher.rs:885, 958`). The first two are centred, the third is not.

### H. Text fields

**H38. Five field chromes for five modals.**

| Field | Chrome | Focus reaction | Size |
|---|---|---|---|
| Input modal (`confirm.rs:290`) | none — icon + bare `Input` | none | TEXT_TITLE |
| `field()` (`add_project.rs:543`) | BG_RAIL, RADIUS_CONTROL, 1px border | ✅ MAGENTA border | TEXT_TITLE |
| Theme editor (`theme_picker.rs:738`) | BG, RADIUS_GROUP, 1px BORDER | **none** | inherited |
| `field_underline()` (`components.rs:838`) | bottom rule only | ✅ MAGENTA rule | TEXT_BODY |
| Palette search (`launcher.rs:722`) | none | none | inherited |

**H39. The most-typed-into field has no focus state.** The theme JSON editor
(`theme_picker.rs:738-750`) is the one multiline buffer in the app and the only
bordered field that never indicates focus.

**H40. `Input`'s built-in inset is zeroed in only one file.** ScriptsEditor zeroes
`pl/pr/py` (`settings.rs:1467, 1598`) with a comment explaining that `Input` applies
its own 10px/8px regardless of `.appearance(false)`. `field()` (`add_project.rs:561`),
the theme editor (`theme_picker.rs:747`) and the palette search (`launcher.rs:732`)
don't — so those three carry an invisible extra inset the ScriptsEditor fields don't.

**H41. ThemeManager's rename row is a non-functional field.** It renders a static
`mono` run plus a 1px div drawn to look like a caret (`theme_picker.rs:869-876`), and
its Cancel button is wired to `ThemeRenameStart` because no cancel click variant
exists (documented at `:890-903`). It looks like a text input and cannot be typed into.

### I. Checkboxes and toggles

**I42. Settings checkboxes render an empty label.** Every Settings checkbox passes
`""` and puts the real label in the row's own label column (`settings.rs:851, 868,
979, 1038`), so `modal_checkbox`'s label + `SPACE_LG` gap machinery
(`components.rs:518-524`) renders a zero-width child. Everywhere else
(RemoveProject `project.rs:603`, AddProject `:762`, ThemePicker `:656`) the checkbox
carries its own label. Two contradictory usages of one component.

**I43. Checkbox accent varies per row with no rule.** CYAN (follow-system),
MAGENTA (project themes, telemetry, init-git), BLUE (Claude in Chrome),
RED (delete worktrees). Only RED (destructive) is explicable.

### J. Status marks

**J44. Three hand-rolled hollow dots.** `status_dot` only produces filled dots
(`components.rs:771`), so "absent" is drawn as
`status_dot(DOT_MD, transparent).border_1()` in Tools rows (`settings.rs:1145`) and as
a raw `div().size(DOT_SM).rounded_full().border_1()` in script rows
(`settings.rs:1581`). No `status_dot_hollow` exists despite two call sites and a
documented §2.3 rationale.

**J45. Dot size splits by file, not by meaning.** Tools rows use DOT_MD; script
rows, archive-gate session rows and Onboarding env rows use DOT_SM — all four are
"this thing is present / running".

### K. Behaviour that shows up as visual drift

**K46. `theme_picker_cancel` is dead code, so cancelling the theme picker leaks the
preview.** `theme_picker.rs:405` is defined to restore `original` and call
`ThemePreview::clear` before leaving — **nothing calls it**. `ModalClick::Cancel` and
the Escape verdict both route to `ModalLayer::cancel` → `ModalSlot::cancel`
(`mod.rs:249`, `modal.rs:649`). `ThemePreview::clear` runs only on submit
(`theme_picker.rs:400`). Cancelling therefore leaves the previewed theme applied and
the `ThemePreview` global set — the picker's own doc comment describes behaviour the
app does not have.

**K47. Return-to-parent isn't signalled consistently.** Changelog's dismiss button
says "Back to Settings" (`settings.rs:1857`) for its return-to-Settings gesture;
ThemePicker performs the identical return (`modal.rs:682-691`) with a button labelled
"Cancel" and no footer hint at all.

---

## 3. What the conformance tests already cover — and don't

`src/views/conformance.rs` greps the view layer for eight rules (bare numeric
literals in styling calls and size arguments, one border weight, font pinning,
display tiers, mono-only tracking, pictographic literals, `CONTROL_H` functions).

None of the 47 findings above are catchable by it: every one is a *component or token
selection* that is individually legal. R1/R6 would pass `MODAL_W_SM` on a modal with
a scrolling list (A3), `TEXT_TITLE` where `body_text` belongs (D14), and a
hand-rolled row that happens to use tokens (G31). The tests guard the scales; nothing
guards which notch a call site picks.

The three cheapest mechanical additions, if you want coverage rather than a one-off sweep:

1. A test asserting every `modal_panel(` call in `src/views/modals/` passes a
   `MODAL_W_*` identifier (kills A1, and forces A2/A3/A4 to be argued explicitly).
2. A test asserting every panel that renders `modal_footer_hints` also renders a
   header, and vice versa (kills F25).
3. A test that `ui(` with `TEXT_TITLE` never appears as a direct child of
   `modal_body(` (pushes D14 onto `body_text`).
