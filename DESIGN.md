# Grove Design System

Design tokens, the rules for using them, and the component contracts they back.

---

## 1. Purpose and how to use this document

This is the reference for Grove's **visual and interaction language**: the token
scales, what each token means, which component owns which decision, and the
invariants that hold the whole thing together. It deliberately says nothing
about product surfaces, feature inventory, or roadmap — those churn, tokens do
not. If you are looking for what a screen *does*, read the screen.

Everything here is traceable to code on the `gpui-rewrite` branch. If a rule is
not in the tree, it is listed as a limitation (§15), never as a spec.

**Citations name a file and a symbol, never a line number.** Line numbers rot on
the first refactor; a function, const or section name survives code moving
within its file. If a cited symbol is gone, that is a real signal that the rule
needs re-checking — which a stale line number would have hidden.

**One paragraph of orientation, and no more.** Grove is a native desktop app
built on gpui. Its window is a fixed chrome stack — an appbar, a resizable
sidebar rail, a session header, a statusbar — wrapped around a dominant canvas
of embedded PTYs. That single fact is what makes the rest of this document
shaped the way it is: the chrome is a frame around live terminal output that
Grove does not own and must not restyle.

**How to use it.** Building a screen: read §3–§9. Adding a token or a
component: read §14 first. Reviewing a diff: §13 is the checklist.

---

## 2. Design principles

A principle that settles no argument is not a principle. Each of these is a
constraint, and each names what it costs.

### 2.1 App chrome must not compete with PTY content

Grove renders terminal bytes faithfully — ANSI colours, cursor, selection — and
never restyles them. The chrome therefore has to stay quiet enough that PTY
output remains the loudest thing on screen.

**Cost:** the app cannot enforce visual consistency inside the canvas. An
agent's own colours will clash with the active theme sometimes, and that is
correct behaviour. It also means the terminal grid opts *out* of the type scale
and the zoom/rem pipeline entirely (§5.5, §6.3) — two systems instead of one.

### 2.2 State is carried by a glyph or a dot, never by a panel

Running state is a filled dot plus a count (`src/views/statusbar.rs` —
`statusbar`'s running group).
Per-session state is a single glyph in a fixed 14px slot
(`src/views/rows.rs` — `state_glyph`).

**Cost:** at-a-glance richness. No badges with three numbers, no status cards,
no sparklines. State that will not fit into a dot, a glyph, or a short mono
count has to be deferred to a modal or dropped.

### 2.3 Colour is never the sole carrier of state

Every state signal pairs colour with a second channel — a glyph shape, a count,
a word, a pulse (full table in §12).

**Cost:** the accent vocabulary is small and each accent is spoken for. You
cannot introduce a new state by picking a new colour; you must find a glyph
too, and the glyph is what has to be legible.

### 2.4 Nothing reflows on hover or on a state change

Hover-revealed controls occupy their slot even while hidden; `state_glyph`
holds a fixed 14px box whichever state renders, and `WaitingForInput` *dims*
rather than hides (`src/views/rows.rs` — `state_glyph`).

**Cost:** rows are permanently as wide as their busiest state, so the resting
layout is looser than it could be. Reserving is still cheaper than a tree that
twitches under the pointer sixteen times a second.

### 2.5 Motion signals, it never decorates

Permitted: a spinner meaning work is in flight, a pulse meaning attention is
needed, a cursor blink meaning a PTY is live. Not permitted: entrance
animations on chrome, hover transitions, easing on colour changes.

**Cost:** the app feels plainer than a consumer product. It also constrains
*how* things animate — alpha only, never a dimension (§11.3).

### 2.6 Components consume roles, never colours

A component may name a tier-2 token (`c::FG_DIM()`, `c::BG_HOVER()`); it may
never name a tier-1 theme colour, an RGB value, or an `Hsla` literal.

**Cost:** you cannot pick "the right blue" for one component. If the role does
not exist you either reuse an existing role or add one to `src/theme.rs`, which
changes all ~40 bundled themes at once — a system-level decision, not a
component-level one.

The one sanctioned exception is a surface whose *subject* is the palette
itself — the theme manager's swatch strip (§4.4).

### 2.7 The numbers are relational, so they must come from a scale

`SPACE_MD` means "one notch of inline breathing room", not "6px". Substituting
one scale token for its neighbour is a legitimate design move; typing `6.0` is
not.

**Cost:** friction at the call site every time the scale is nearly right. That
friction is the point — it is what keeps the scale from silently growing a
fifth spacing value per screen.

---

## 3. Token architecture

Three tiers, strictly one-directional.

```
grove-core::theme::Theme          tier 1  — 11 semantic roles, per theme
        ↓ read via theme::with_current
src/theme.rs        (as `c::`)    tier 2  — derived colour tokens + _of() variants
src/views/tokens.rs               tier 2  — numeric scales (space/type/icon/dot/
                                            modal-width/radius/height)
src/views/dispatch.rs             tier 2  — the click-intent vocabulary
        ↓ consumed by
src/views/components.rs           tier 3  — the shared component library
src/views/{appbar,sidebar,…}.rs           — screens
src/views/modals/*.rs                     — modal screens; interpret dispatch
```

**Tier 1 — grove-core theme.** A flat `Theme` struct: `bg`, `bg_highlight`,
`fg`, `fg_dark`, `comment`, six accents (`blue`, `cyan`, `magenta`, `green`,
`yellow`, `red`), and a `kind` discriminant (Dark/Light). Eleven roles. This is
the only thing a theme author writes.

**Tier 2 — derived tokens.** `src/theme.rs` synthesizes the richer surface
vocabulary the GUI needs (rail, strip, hover, two border weights, scrim,
selection tints) by blending tier-1 colours at fixed ratios. Every accessor
reads the *active* theme on each call, so a theme swap takes effect on the next
frame with no cache invalidation. A token read costs an atomic load, not a lock
(`src/theme.rs` — module doc).

`src/views/tokens.rs` is the numeric half of tier 2 — spacing, type sizes, icon
glyph sizes, dot sizes, modal panel widths, radii, control height. No colour.

`src/views/dispatch.rs` is the third piece of tier 2: `ModalClick`,
`SettingToggle` and `ModalDispatch` — a click *intent* vocabulary, no
presentation. It sits below `components.rs` on purpose. A shared button needs
to be wirable to an intent, and if that vocabulary lived in the modal layer the
app-wide library would import from a screen. `modals/mod.rs` re-exports the
three names for the screens that were already spelling them
`modals::ModalClick` (`src/views/modals/mod.rs` — the `pub use` of
`crate::views::dispatch`).

**Tier 3 — components.** `src/views/components.rs` plus the per-region view
modules. Pure presentation: every helper in `components.rs` takes plain data
and returns an element — no entity, no `Context` (`src/views/components.rs` —
module doc).

### The seam rule

**Swapping a theme must never require touching a component**, and
`src/theme.rs` must never learn what a component is. `theme.rs` does not export
"button background"; it exports `BG_HL`, and the button decides that is what it
wants.

The same rule points downward as well: **`components.rs` may not import from a
screen.** It reads `theme.rs`, `tokens.rs`, `dispatch.rs`, `icons.rs` and
`fonts.rs`, and nothing in `views/modals/` or the per-region views. That
constraint is what `dispatch.rs` exists to satisfy. The library now imports no
screen module at all.

The one sanctioned violation in the tree is the modal panel's drop shadow —
fixed black at α 0.35, because a shadow is an optical effect rather than a
surface (`src/views/components.rs` — `modal_panel`).

---

## 4. Colour tokens

### 4.1 Tier 1 — the eleven roles

| Role | Meaning |
|---|---|
| `bg` | Workspace canvas, base surface |
| `bg_highlight` | Active rows, selected controls, filled chips |
| `fg` | Primary text, active icons |
| `fg_dark` | Secondary text |
| `comment` | Muted text, low-priority metadata |
| `blue` | Navigation and informational accents |
| `cyan` | Selection and secondary accent |
| `magenta` | Agent / category accent |
| `green` | Running state, positive actions |
| `yellow` | Warnings, pending state |
| `red` | Destructive actions, errors |

Plus `kind: Dark | Light`, which several derivations branch on.

### 4.2 Tier 2 — derived tokens

All in `src/theme.rs`, imported everywhere as `use crate::theme as c;`. Every
`SCREAMING_CASE` accessor returns `gpui::Hsla`. The two lower-level public
helpers do not: `ic(color)` converts a grove-core `Color` to `Rgba` (it is the
conversion the derivations are built on, public so a theme editor can paint a
draft theme — §4.4), and the `_of(theme)` variants return `Rgba` (§4.4).

#### Surfaces

| Token | Derivation | Role | Representative use |
|---|---|---|---|
| `BG()` | `theme.bg` | Body canvas, PTY backdrop, unfilled controls | workspace body, empty states, `ModalBtn::Plain` fill |
| `BG_RAIL()` | dark `mix(bg, black, 0.18)`; light `mix(bg, black, 0.04)` | Sidebar rail, modal panel fill | `src/theme.rs` — `BG_RAIL`; `modal_panel` |
| `BG_STRIP()` | dark `mix(bg, black, 0.32)`; light `mix(bg, black, 0.08)` | Outer chrome edge — the darkest surface | `footer_container`, statusbar, ANSI colour 0 |
| `BG_HOVER()` | `mix(bg, bg_highlight, 0.55)` | Every hover fill in the app | all buttons, rows, segments |
| `BG_HL()` | `theme.bg_highlight` | Active/selected row, filled keycaps, active segment | `keycap`, `click_row`, `seg_button_content` |
| `BORDER()` | `mix(bg, fg, 0.16)` | Standard 1px stroke | panel, buttons, `seg_group`, `vline` |
| `BORDER_SOFT()` | `mix(bg, fg, 0.07)` | Quieter rule *inside* a panel | `divider_h`, inter-tile gap, statusbar top rule |

**Surface luminance ordering is a load-bearing invariant:**

```
luminance(BG_STRIP) < luminance(BG_RAIL) < luminance(BG)
```

asserted by `chrome_surfaces_get_progressively_darker` (`src/theme.rs`, test
module).
The chrome reads as depth *because* of this ordering; a derivation change that
breaks it fails the test suite. It holds on light themes too — light themes
still mix toward black, just at a shallower ratio (0.04 / 0.08 vs 0.18 / 0.32).

#### Overlay

| Token | Derivation | Role |
|---|---|---|
| `SCRIM()` | dark `mix(bg, black, 0.9)` @ α 0.16; light `mix(bg, fg, 0.9)` @ α 0.16 | Modal backdrop |

The light branch dims toward the *foreground*, not black, so the wash stays
visible on near-white backgrounds (`src/theme.rs` — `SCRIM`). gpui has no backdrop
blur at this rev, so this is a flat theme-derived wash.

#### Text

| Token | Derivation | Role |
|---|---|---|
| `FG()` | `theme.fg` | Primary text, active icons, primary button label |
| `FG_DIM()` | `theme.fg_dark` | Body prose, secondary labels, icon-button rest state |
| `FG_MUTE()` | `theme.comment` | Section headers, captions, inert glyphs, disabled text |

#### Accents

| Token | Derivation | Semantics |
|---|---|---|
| `GREEN()` | `theme.green` | **Running.** Running dot, `Working` spinner, `main` tag, upgrade-available dot |
| `AMBER()` | `mix(yellow, red, 0.25)` | **Needs you.** Warmer than YELLOW so it reads as a call to action next to green/working (`src/theme.rs` — `AMBER`, `amber_rgba`) |
| `YELLOW()` | `theme.yellow` | Warning / pending. **Not** the attention colour |
| `RED()` | `theme.red` | **Destructive or error.** `ModalBtn::Danger`, `note_text`, danger segments |
| `CYAN()` | `theme.cyan` | **Selection.** Selection tints and ring, palette cue chips, active grid segment |
| `BLUE()` | `theme.blue` | **Navigation / informational** |
| `MAGENTA()` | `theme.magenta` | **Agent / category.** The `grove` brand mark, the launcher's new-session rows |

`amber_sits_between_yellow_and_red` (`src/theme.rs`, test module) asserts AMBER stays
inside the yellow→red interval and nearer yellow.

#### Washes and selection

| Token | Derivation | Role |
|---|---|---|
| `RED_WASH()` | `mix(red, bg, 0.84)` — a 16% red wash | Active fill for a *danger-flavoured* segment, distinct from neutral `BG_HL` |
| `SEL_TINT_STRONG()` | `cyan` @ α 0.22 | Fill for a row in **edit/rename** mode |
| `SEL_TINT_SOFT()` | `cyan` @ α 0.10 | Fill for an **ordinary selected** row |
| `SEL_RING()` | `cyan` @ α 0.50 | The 1px ring outlining a selected row at **either** weight |
| `AMBER_ROW_TINT()` | `alpha(AMBER(), 0.12)` | Fill behind a row whose session is **waiting on you**. Faint enough to keep the row's text contrast; the glyph still names the state (§2.3), the tint only locates it. Live in `src/views/rows.rs` and `src/views/modals/launcher.rs` — two consumers, which is why it is a token and not a module constant (§14) |

**Selection has two intentional weights.** They are not a gradient and not a
leftover:

| Weight | Fill | Ring | Meaning | Live at |
|---|---|---|---|---|
| Soft | `SEL_TINT_SOFT` (α 0.10) | `SEL_RING` | This row is selected | `src/views/components.rs` — `palette_row` |
| Strong | `SEL_TINT_STRONG` (α 0.22) | `SEL_RING` | This row is *in edit/rename mode* — one weight up | `src/views/modals/theme_picker.rs` — `manager`'s rename-edit row |

The ring is constant across both; the **fill alpha** is the channel that
carries the escalation. If you need a third weight, you are almost certainly
looking for a different signal, not a third alpha.

`cue_chip` borrows `SEL_TINT_SOFT` as a fill but is **not** a third weight and
not a selection: it is a mode indicator in the palette's glyph slot, and it
carries no ring (`src/views/components.rs` — `cue_chip`).

### 4.3 Blending rules

Two rules, both non-negotiable:

1. **Blend component-wise on 0..1 sRGB floats (`gpui::Rgba`), converting to
   `Hsla` only at the end of the token function.**
2. **Never blend in HSL space.** HSL interpolation shifts hue as it crosses the
   colour solid; doing it here would visibly change ~40 themes at once
   (`src/theme.rs` — module doc).

`mix(a, b, t)` clamps `t` to `0..1` and preserves `a`'s alpha
(`src/theme.rs` — `mix`).

`pub fn alpha(c: Hsla, a: f32) -> Hsla` (`src/theme.rs` — `alpha`) overrides
alpha and nothing else, so the result still tracks a theme swap. It is public
and it is the **sanctioned** way to tint a tier-2 token at a call site.
Writing `Hsla { a: .., ..c::TOKEN() }` inline is the same operation spelled
ad-hoc and hides the tint site from a grep; do not. If two or more call sites
want the same alpha on the same token, promote it to a named token in
`theme.rs` instead — that is exactly how `AMBER_ROW_TINT` came to exist (§14).

**Cost:** one more indirection between a call site and the colour it paints,
and a token list that grows every time a second consumer appears.

### 4.4 The `_of(theme)` variants

App chrome always reads the *global* active theme. Two kinds of surface do not:

1. **PTY content.** A project may pin a project theme that applies to its
   terminal content only — background fill, default fg, cursor, ANSI 0-15.
2. **Theme previews.** The theme picker and theme manager must paint a theme
   the user has not applied: swatches, sample rows, the preview panel. That
   surface's whole job is to render *some other* theme.

The parameterised variants exist for exactly that decoupling
(`src/theme.rs` — the *theme-parameterized variants* section).

Available: `bg_of`, `fg_of`, `fg_mute_of`, `blue_of`, `cyan_of`, `magenta_of`,
`green_of`, `yellow_of`, `red_of`, `bg_rail_of`, `bg_strip_of`, `bg_hl_of`,
`bg_hover_of`, `border_of`, `sel_ring_of`.

Of these, `border_of`, `bg_hover_of` and `bg_strip_of` are live; `bg_hl_of` and
`sel_ring_of` are **reserved with no consumer yet** — they are kept so a
preview surface that grows a selected row does not have to re-derive one
(`src/theme.rs` — `bg_hl_of`, `sel_ring_of`).

They return `Rgba`, not `Hsla`, because they are blend inputs to one another;
the painting call site converts with `.into()`.

**Rule:** an element renders *one* theme. Chrome uses the bare accessors; PTY
content and preview surfaces use `_of()` throughout, `border_of` included.
Never mix the two in one element — a border from the active theme around a fill
from a previewed one is the exact bug this rule exists to prevent.

**The swatch strip is a sanctioned exception to §2.6.** `swatch_strip`
(`src/views/modals/theme_picker.rs` — `swatch_strip`) walks
`grove_core::theme::FIELD_NAMES` and paints each field with `c::ic(t.field(i))`
— raw tier 1, by index, named nowhere. This is not a component picking a
colour: the eleven roles *are* the content, and a palette preview that showed
derived tokens instead would be showing something the theme author cannot edit.
It is listed here rather than in §15 because there is no better shape to
migrate to — a `_of()` accessor per role would be the same read with more
names. The exception is scoped to preview surfaces; no other component may
reach tier 1.

### 4.5 Theme resolution

`ThemeState` (`src/theme.rs`) holds *policy*, not colours — grove-core's
`theme::ACTIVE` stays the single source of truth. Defaults: `tokyonight-storm`
(dark) / `tokyonight-day` (light). Under `follow_system`, any appearance that is
not explicitly Light resolves dark. `generation` bumps on every change and is
the terminal element's repaint/cache key.

---

## 5. Typography

### 5.1 Two families, no more

| Constant | Value | Use |
|---|---|---|
| `fonts::UI_FAMILY` | `IBM Plex Sans` | Prose, titles, button labels, row labels |
| `fonts::MONO_FAMILY` | `BlexMono Nerd Font Mono` | PTY content, keycaps, section labels, counts, data |

Both are bundled as regular + bold TTFs and registered before the window opens
(`src/fonts.rs` — `FONT_FILES`, `register`). No italics (§13).

### 5.2 Mono vs sans

- **Mono** for anything read as a *token*: keycaps (`⏎`, `esc`, `↑↓`),
  letter-tracked section headers, numeric counts, segment labels, cue chips.
- **Sans** for anything read as *language*: modal titles, body prose, captions,
  button labels, row names.

There is exactly one primitive for each, both in the shared library:
`components::ui(content, size, color)` and
`components::mono(content, size, color)` (`src/views/components.rs` — `ui`,
`mono`).
No module keeps a private copy.

### 5.3 The scale

`src/views/tokens.rs`, in design pixels, fed to `rpx()`.

| Token | Value | Use |
|---|---|---|
| `TEXT_MICRO` | 10 | Section headers, footer hints, mono captions, counts, `main` tag |
| `TEXT_SMALL` | 11 | Keycap labels, segment labels, captions, validation notes |
| `TEXT_BODY` | 12 | Body prose, button labels, checkbox labels, row labels |
| `TEXT_TITLE` | 13 | Modal titles, appbar icon glyphs, empty-state titles |
| `TEXT_DISPLAY` | 20 | **Empty-state / onboarding only** |
| `TEXT_DISPLAY_LG` | 32 | **Empty-state / onboarding only** |

The two display tiers are **never chrome** — the doc comment on
`TEXT_DISPLAY` in `src/views/tokens.rs`
says so, and it is a rule, not a note. Four chrome tiers is the whole
vocabulary. If a design needs a fifth, the design is wrong.

### 5.3.1 The icon scale

Icon glyphs are sized from their own scale in `src/views/tokens.rs`, fed to
`icons::icon` / `icons::spinner`, which apply `rpx` internally (§9.3).

| Token | Value | Use |
|---|---|---|
| `ICON_XS` | 10 | The smallest legible mark — inline chips, footer hints, statusbar and tile-header glyphs |
| `ICON_SM` | 12 | List density — row state glyphs, menu items, the modal close button |
| `ICON_MD` | 14 | Chrome glyphs — appbar, session header, term panel, settings rows |
| `ICON_LG` | 16 | The largest clickable glyph — palette rows, confirm-modal marks. The sprite's native `viewBox` |
| `ICON_DISPLAY` | 32 | **Empty-state / onboarding only** |

`ICON_DISPLAY` is **never chrome**, on the same terms as `TEXT_DISPLAY` (§5.3):
a rule, not a note. Four chrome tiers is the whole vocabulary here too.

**There is no 11px tier.** The glyphs that used to be authored at 11 round *up*
to `ICON_SM`; the scale steps 10 / 12 / 14 / 16 and a one-pixel notch inside it
would not be a design decision anyone could defend at a review.

### 5.3.2 Activity dot sizes

| Token | Value | Use |
|---|---|---|
| `DOT_SM` | 6 | Dots inside a list row or a tab label |
| `DOT_MD` | 7 | Dots standing alone in a bar — statusbar, appbar pill, settings rows |

Two sizes, because a dot with a label beside it and a dot carrying a bar on its
own need different weight. Both feed `components::status_dot` (§9.1); neither
is a radius — see `RADIUS_FULL` (§7.1).

### 5.4 Letter tracking

gpui has no letter-spacing property. Tracking is faked by joining every
character with U+2009 THIN SPACE:

```rust
// src/views/components.rs — tracked
pub fn tracked(label: &str) -> String {
    label.chars().map(String::from).collect::<Vec<_>>().join("\u{2009}")
}
```

`section_header` (`src/views/components.rs`) no longer calls `tracked()` on
its own label — faked tracking on an uppercase mono run read as "split-out"
text rather than actual tracking, so the label now renders plain. `tracked()`
itself is unchanged and still the only sanctioned way to fake tracking where a
caller genuinely wants it: mono, uppercase text only. Never apply it to
prose — it destroys word shape and breaks copy/search.

### 5.5 PTY text is outside the type system

**Hard rule.** Terminal content uses `fonts::FONT_SIZE` (12.5),
`fonts::CELL_W` (7.5) and `fonts::CELL_H` (17.0), and nothing else. Not the
type scale, not `rpx`, not `window.line_height()` (26px), not
`window.rem_size()` (16px) — `CELL_H` is explicitly "a GROVE constant, NOT a
font metric" (`src/fonts.rs` — `CELL_H`).

**Why they are authored constants rather than measured.** The grid maps
`(row, col)` straight to pixels; a measured value drifting by a fraction of a
pixel would silently walk the cursor off-position across a long row. So the
constants are fixed and the *font* is verified against them at startup:
`assert_cell_metrics` shapes an "M" in `MONO_FAMILY` at `FONT_SIZE` and
requires the advance within `CELL_W_EPSILON` (0.001px) of 7.5
(`src/fonts.rs` — `assert_cell_metrics`). The epsilon is 0.001 because float noise measures
~5e-7 while a wrong font or size is off by ≥0.3px per cell. On failure the
process exits — "a shell that renders a subtly-misaligned grid is worse than
one that refuses to start" (`src/fonts.rs` — `register_and_assert_or_exit`).

Zoom scales all three by multiplication (`src/zoom.rs` —
`ZoomState::cell_w`, `cell_h`, `font_size`) — a separate pathway from the rem pipeline.

---

## 6. Spacing and the `rpx` unit

### 6.1 The scale

`src/views/tokens.rs`, in design pixels:

| Token | Value | Typical use |
|---|---|---|
| `SPACE_XS` | 2 | Keycap/chip vertical padding |
| `SPACE_SM` | 4 | Segment vertical padding, menu-row vertical padding |
| `SPACE_MD` | 6 | Default inline gap — icon↔label, keycap↔label, dot↔count; keycap h-padding |
| `SPACE_LG` | 8 | Row gap and h-padding, footer vertical padding, list-row gap |
| `SPACE_XL` | 10 | Modal body vertical rhythm, caption indent |
| `SPACE_2XL` | 12 | Button h-padding, palette-row h-padding, section-label indent |
| `SPACE_3XL` | 16 | Modal zone padding (header / body / footer sides) |

**Rule: no bare numeric literal in a styling call.** `.px(rpx(8.0))` is wrong;
`.px(rpx(SPACE_LG))` is right. This applies to spacing, type size, radius and
control height alike. The legitimate exceptions are enumerated in §14.

### 6.2 What `rpx` is

```rust
// src/views/mod.rs — rpx
pub fn rpx(v: f32) -> Rems { rems(v / crate::zoom::REM_BASE) }
```

A **design pixel expressed in rems**. The chrome is authored in legible pixel
numbers, but `px()` values are immune to `Window::set_rem_size` — only `rems()`
scales. `rpx(12.)` keeps the readable number *and* zooms, because the root view
sets the rem size once per frame.

### 6.3 The three exclusions — where `rpx` is wrong

1. **1px hairlines.** A hairline is a hairline at every zoom. Use `px(1.0)`:
   `divider_h` and `vline` (`src/views/components.rs`), the inter-tile gap
   (`src/views/grid.rs` — `grid`), and
   `modal_panel`'s 1px content
   inset.
2. **Physical window / viewport math.** Mouse positions, window bounds,
   `Bounds::centered` — device-space quantities, not styling.
3. **The terminal element's own cell grid**, which has its own zoom pathway
   (§5.5).

### 6.4 How zoom works

`src/zoom.rs`:

| Constant | Value |
|---|---|
| `REM_BASE` | 16.0 (gpui's default rem size) |
| `ZOOM_DEFAULT` | 1.0 |
| `ZOOM_MIN` | 0.6 |
| `ZOOM_MAX` | 2.0 |
| `ZOOM_STEP` | 0.1 |

The root view calls `set_rem_size(px(REM_BASE * zoom))` once per frame, so all
`rems()`-styled chrome scales from that single call. `snap()` clamps, rounds to
the 0.1 grid, then clamps *again* — the second clamp matters because rounding
can push a clamped endpoint back out of range.

PTY dimensions come from the terminal element's own post-layout bounds in
`prepaint` (`cols = floor(width / cell_w)`), never by subtracting chrome: gpui
layout has already excluded it (`src/zoom.rs` — `ZoomState::pty_dims`).

`pty_dims` floors, then clamps at **both** ends: degenerate bounds — zero,
negative, NaN, or smaller than one cell — give a 1×1 grid, and absurdly large
bounds saturate at `u16::MAX` rather than wrapping. A PTY may never be sized 0,
and it may never be sized by a wrapped integer either (`src/zoom.rs` —
`pty_dims_clamp_degenerate_bounds`, `pty_dims_never_exceed_u16`).

---

## 7. Geometry

### 7.1 Radius scale

`src/views/tokens.rs`:

| Token | Value | Applies to |
|---|---|---|
| `RADIUS_CONTROL` | 4 | Buttons, keycaps, checkboxes, list rows, icon buttons, cue chips, segment outer corners |
| `RADIUS_GROUP` | 6 | Segmented-control wrapper (`seg_group`), palette rows, the theme picker's edit row |
| `RADIUS_PANEL` | 12 | Modal panels |
| `RADIUS_FULL` | 999 | Dots and pills |

Rule of thumb: the bigger the surface, the bigger the radius. A control gets 4,
a group of controls gets 6, a floating panel gets 12.

`RADIUS_FULL` and gpui's `rounded_full()` are **both acceptable and mean the
same thing**; neither is preferred. The tree uses `rounded_full()` in
`status_dot` (`src/views/components.rs` — `status_dot`) and `rpx(RADIUS_FULL)`
in the appbar's pills (`src/views/appbar.rs` — `attention_pill`,
`zen_attention_pill`), and there is no rule that would move either. Do not
"fix" one into the other.

### 7.2 Border weights

Exactly one weight: **1px hairline**, always `px(1.0)`, never `rpx`. Applied via
gpui's `.border_1()` (already a device pixel) or an explicit `div().h(px(1.0))`
/ `.w(px(1.0))` for rules.

Two tones: `BORDER()` for structural strokes, `BORDER_SOFT()` for rules *inside*
a panel where a full-strength stroke would over-segment the content.

### 7.3 The inset-corner rule

A filled child sitting 1px inside a rounded parent must round 1px *tighter* to
trace the same arc. `modal_panel` applies `.p(px(1.0))` so its filled footer
strip stays inside the border stroke rather than painting over it, and the
footer therefore uses:

```rust
// src/views/components.rs — FOOTER_RADIUS
const FOOTER_RADIUS: f32 = RADIUS_PANEL - 1.0;   // = 11
```

This is a *derived* value. If `RADIUS_PANEL` moves, `FOOTER_RADIUS` moves with
it; it is never an independent choice.

The same relationship appears in the segmented control, where a segment's outer
corners round at `RADIUS_CONTROL` (4) inside a `seg_group` bordered at
`RADIUS_GROUP` (6) (`src/views/components.rs` — `seg_button_content`,
`seg_group`).

---

## 8. Layout constants

### 8.1 Chrome stack and row heights

| Constant | Value | Owner |
|---|---|---|
| `appbar::APPBAR_H` | 44 | `src/views/appbar.rs` |
| `session_header::SESSBAR_H` | 36 | `src/views/session_header.rs` |
| `statusbar::STATUS_H` | 26 | `src/views/statusbar.rs` |
| `rows::ROW_H` | 28 | `src/views/rows.rs` |
| `components::PALETTE_ROW_H` | 54 | `src/views/components.rs` |
| `grid::TILE_HEAD_H` | 22 | `src/views/grid.rs` |
| `tokens::CONTROL_H` | 22 | `src/views/tokens.rs` |
| `launcher::AGENT_BTN` | 26 | `src/views/modals/launcher.rs` — the agent bar's square icon box |

A worktree row showing a branch chip is `ROW_H + 14 = 42` tall
(`src/views/rows.rs` — `row_height`). The agent-menu overlay position
is computed from that function, so the renderer and `row_height` must agree —
changing one without the other misplaces the menu.

`PALETTE_ROW_H` (54) is deliberately taller than `ROW_H` (28): palette rows
carry two lines of content.

`CONTROL_H` (22) is the height of every flat icon/text button and equals
`TILE_HEAD_H`, which is why a tile header and a chrome button read as the same
weight.

A settings row (App Settings and Project Settings both, since the
Settings-modal unification gave them one shared row grid) is sized by
**padding around its content**, not by a pinned height. A pinned height makes
content either rattle inside a box built for something taller, or spill out of
a box built for something shorter — the settings rows used to manage both at
once, with a fixed `FIELD_H`/`FIELD_H_TALL` pair that assumed every row was
either exactly one line or exactly two. §9.1's `RowDensity::Card` already
documents the right model — it "takes its height from its content rather than
from padding" — and the settings rows were the exception to that precedent,
not a second rule. They now follow it:

| Constant | Formula | Value | Owner |
|---|---|---|---|
| `ROW_PX` | `SPACE_2XL` | 12 | `src/views/tokens.rs` — a settings row's horizontal padding, **both** edges. The missing trailing edge, before this token existed, is what let a long script value run flush into the card's border |
| `ROW_PY` | `SPACE_LG` | 8 | `src/views/tokens.rs` — a settings row's vertical padding, top and bottom |
| `ROW_LINE_GAP` | `SPACE_MD` | 6 | `src/views/tokens.rs` — the gap between a row's label line and its sublabel line |
| `ROW_MIN_H` | `CONTROL_H + ROW_PY * 2` | 38 | `src/views/tokens.rs` — a row's height **floor**, expressed as `min_h` only and never as `h()`: a row containing a `CONTROL_H` (22) control can never collapse smaller than that control fits |

A row's real height falls out of its content plus `ROW_PY`, not out of a
lookup by row kind: a single-line row measures `ROW_MIN_H` (38); a
label+sublabel row measures `ROW_PY + CONTROL_H + ROW_LINE_GAP + TEXT_SMALL +
ROW_PY` (55); and a sublabel that wraps to two lines grows the row instead of
overflowing it. There is no tall variant to pick — tallness is just more
content inside the same padding contract.

One more constant is derived from `ROW_MIN_H` rather than authored:

| Constant | Formula | Value | Owner |
|---|---|---|---|
| `MODAL_SCROLL_MAX_H` | `ROW_MIN_H * 12` | 456 | `src/views/tokens.rs` — the settings body's scroll cap, both modals |
| `CARD_LABEL_INDENT` | `SPACE_2XL + 1` | 13 | `src/views/tokens.rs` — a settings card's label indent, derived from the card's own 1px border plus a row's `SPACE_2XL` inset |
| `STATUS_DOT_COL_W` | `DOT_MD + SPACE_SM * 2` | 15 | `src/views/tokens.rs` — the leading status-dot column reserved on every settings row |

Plus `FIELD_LABEL_COL_W` (92, `src/views/tokens.rs`) — the fixed label-column
width shared by every field row in Project Settings, so three fields align on
one edge regardless of label length.

**All of these must stay derived, never re-authored as a literal.** Each
formula names the failure mode a hardcoded value would reintroduce:

- `MODAL_SCROLL_MAX_H` is a **maximum on the body, not a layout**: rows vary in
  height now, so it can no longer be read as "N whole rows" the way a
  fixed-height design could. If it were re-authored as a bare pixel number
  instead of `ROW_MIN_H * 12`, the moment row geometry moved again a
  hardcoded cap would silently clip the first Tools row out of the scrollable
  body rather than growing with it — the same failure this token exists to
  rule out.
- `CARD_LABEL_INDENT` tracks a settings row's own horizontal padding. A card
  label positioned by an independent literal drifts out of true against the
  rows it names the moment that padding changes — a 1px misalignment that
  reads as sloppiness rather than as a bug, because nothing crashes.

These are the same "derived, not chosen" discipline as `FOOTER_RADIUS` (§7.3):
the number is correct only because of what it is computed from, and moves the
instant its input does.

**This is a rule about content rows inside panels, not about chrome.** The row
heights in the table at the top of this section — `APPBAR_H`, `SESSBAR_H`,
`STATUS_H`, `ROW_H`, `PALETTE_ROW_H`, `TILE_HEAD_H` — stay fixed on purpose,
because alignment across a bar depends on every row in it landing at the same
pixel. A settings row inside a card has no neighbour bar to align against; a
worktree row in the sidebar does. Do not carry the padding contract above
into chrome, and do not carry a chrome bar's fixed height into a panel's
content rows — each was the fix for a different failure.

### 8.2 Panel widths

| Constant | Value | Owner |
|---|---|---|
| `RAIL_W` | 320 | Default sidebar width — `src/entities/workspace_state.rs` — `RAIL_W` |
| `SIDEBAR_MIN_W` | 220 | Drag floor — `src/entities/workspace_state.rs` — `SIDEBAR_MIN_W` |
| `WORKSPACE_MIN_W` | 400 | The body may never be squeezed below this — `src/entities/workspace_state.rs` — `WORKSPACE_MIN_W` |
| `SIDEBAR_DIVIDER_W` | 6 | Drag handle hit area — `src/views/sidebar.rs` — `SIDEBAR_DIVIDER_W` |

The divider is 6px of *hit area* around a 1px visual hairline. Widening the
visual line to match would read as a panel edge rather than a handle.

The body/terminal-panel split is a **percentage**, not a width, because it has
to survive a window resize:

| Constant | Value | Owner |
|---|---|---|
| `TERM_PANEL_PORTION` | 40 | Default share of the body, in percent — `src/entities/workspace_state.rs` |
| `TERM_PANEL_PORTION_MIN` | 20 | Floor |
| `TERM_PANEL_PORTION_MAX` | 75 | Ceiling |
| `TERM_PANEL_PORTION_STEP` | 5 | One keyboard nudge |

### 8.2.1 Truncation limits

| Constant | Value | Owner |
|---|---|---|
| `session_header::CONTEXT_MAX_CHARS` | 80 | Middle-truncation budget for the session header's context title — `src/views/session_header.rs` |

A character budget, not a pixel width: the header truncates *middle-out* so the
head and tail of a path both survive, which a pixel ellipsis cannot express.

### 8.3 PTY padding

| Constant | Value | Owner |
|---|---|---|
| `PTY_PAD_W` | 36 | Single-session canvas, horizontal — `src/views/grid.rs` — `PTY_PAD_W` |
| `PTY_PAD_H` | 28 | Single-session canvas, vertical — `src/views/grid.rs` — `PTY_PAD_H` |
| `TILE_PTY_PAD_W` | 32 | Grid tile, horizontal — `src/views/grid.rs` — `TILE_PTY_PAD_W` |
| `TILE_PTY_PAD_H` | 24 | Grid tile, vertical — `src/views/grid.rs` — `TILE_PTY_PAD_H` |

Tiles are tighter than the single-session canvas because several share the body;
the padding budget shrinks with the surface.

### 8.4 Why layout geometry is deliberately not tokenised

The scales in `tokens.rs` are **relational** — `SPACE_MD` is "one notch", and
the same notch is right in a statusbar chip and a modal footer.

Layout geometry is **positional and singular**. `APPBAR_H` is not "a large
height", it is *the appbar's* height; `RAIL_W` is not "a wide width", it is
*the rail's* width. There is no scale they belong to, no second consumer they
could migrate between, and hoisting them into a shared module would only
obscure which module owns the number.

`STATUS_DOT_COL_W` (`DOT_MD + SPACE_SM * 2`) and `CARD_LABEL_INDENT`
(`SPACE_2XL + 1`) used to be exactly this kind of example — single-consumer
`const`s local to `src/views/modals/settings.rs` — until the Settings-modal
unification gave each a second consumer (App Settings and Project Settings
now share one row grid, §9.1). A derived value earns a *scale* home in
`tokens.rs` the moment a second surface needs the same number for the same
reason; until then it stays positional and module-local, same as `APPBAR_H`.

**Therefore:** chrome bar heights, rail widths, PTY padding, row heights and
divider widths stay as named `pub const` in the module that owns the surface.
Spacing, type, icon and dot sizes, modal widths, radius and control height live
in `tokens.rs`. Both are tokens in the sense that neither may appear as a bare
literal at a call site; only the latter is a *scale*.

**The test is relational-versus-positional, not layout-versus-not.** `APPBAR_H`
is *the appbar's* height — one surface, one number, no neighbour to compare it
to. A modal width is the opposite: there are a dozen modals, each one picks a
width by asking "wider or narrower than the last one?", and the answer is a
notch on a shared scale. That is why §8.5 lives in `tokens.rs` and this section
does not contradict it.

### 8.5 Modal panel widths

`src/views/tokens.rs`, passed to `components::modal_panel` (§9.1):

| Token | Value | Use |
|---|---|---|
| `MODAL_W_SM` | 420 | Confirmations and single-question modals — one short paragraph, no list |
| `MODAL_W_MD` | 480 | The default: a form, or a short list of rows |
| `MODAL_W_LG` | 560 | Rows carrying a secondary column — a path, a hint, a trailing control — that would truncate at `MODAL_W_MD` |
| `MODAL_W_XL` | 640 | The command palette, Settings, the onboarding wizard: a scrolling results list rather than a form. **Nothing goes above this** |

Four notches replaced nine ad-hoc widths across 23 call sites. Picking a width
is now the same move as picking a spacing notch: if a modal is truncating, it
goes up one tier, and if no tier fits, the modal is carrying too much (§13).

**A fifth notch was tried and retired.** `MODAL_W_LG2` (600) was added to give
Project Settings a shade more room than `MODAL_W_LG`, outside these four. It
was deleted during the Settings-modal unification: it is exactly the "modal is
carrying too much" case this section's closing rule already forbids, and
Project Settings drops to `MODAL_W_LG` (560) instead. The four notches above
are the whole scale, not four of five.

`PALETTE_W` (760) has now been retired the same way. The command palette used
to render at its own private 760px width, wider than every other modal, and
now sits on `MODAL_W_XL` (640) like everything else. This is a real tradeoff,
not a free win: palette rows carry both a title and a subtitle (a "project ·
worktree" style label), so at the narrower 640px, long labels truncate sooner
than they did at 760px.

---

## 9. Components

### 9.1 The shared library — `src/views/components.rs`

This is the **app-wide** component library, not modal chrome. Every public item
is a contract; all are pure functions of plain data — no entity, no `Context`.
It is consumed by `appbar`, `sidebar`, `rows`, `statusbar`, `grid`,
`session_header`, `term_panel`, `terminal_tab` and every modal.

#### Text primitives

| Component | Contract | Fixed tokens |
|---|---|---|
| `ui(content, size, color)` | The single sans text run for the whole app | `UI_FAMILY`; size via `rpx` |
| `mono(content, size, color)` | The single mono text run for the whole app | `MONO_FAMILY`; size via `rpx` |
| `body_text(content)` | Body prose | sans, `TEXT_BODY`, `FG_DIM` |
| `note_text(content)` | Inline validation note under a field | sans, `TEXT_SMALL`, `RED` |
| `caption(content)` | Muted one-liner explaining a control | sans, `TEXT_SMALL`, `FG_MUTE`, `px SPACE_XL` |
| `caption_promoted(content)` | One shade up — **reserved for safety-relevant captions** | sans, `TEXT_SMALL`, `FG_DIM`, `px SPACE_XL` |
| `section_header(label, indent, top, bottom)` | Mono, uppercase section label. `indent` is an axis rather than a fixed `SPACE_2XL`: a label naming a bordered `card` sits flush with the card's edge, while a label over a flat list is inset to the list's text column | mono, `TEXT_MICRO`, `FG_MUTE`, `pl rpx(indent)` |
| `row_sublabel(content, tone)` | The second line under a row's label — no indent of its own, since the row's label column already owns the left edge | sans, `TEXT_SMALL` | `SublabelTone::{Normal,Safety}` |

#### Panel / modal structure

| Component | Contract | Fixed tokens |
|---|---|---|
| `modal_panel(width, content)` | Panel shell: `BG_RAIL` fill, 1px `BORDER`, `RADIUS_PANEL`, shadow (black α0.35, y+12, blur 40), 1px content inset | radius 12, `p(px(1.0))` |
| `scrim(content)` | Full-bleed `SCRIM` backdrop, content centred | — |
| `scrim_top_drop(content)` | Palette variant: top-dropped | `pt(rpx(80))` |
| `modal_header(title, accent)` | Header zone with a `TEXT_TITLE` title in `accent` | `px/py SPACE_3XL` |
| `modal_header_with_close(id, title, accent, dispatch)` | `modal_header_row` with the title left and a trailing `close` icon button wired to `ModalClick::Cancel` | header tokens + `flat_icon_btn` at the header icon tier: `ICON_BTN_W` (28), `ICON_MD` |
| `modal_header_row(content)` | Header zone for a custom row (title + step counter) | `px/py SPACE_3XL` |
| `modal_body(content)` | Body zone: column with vertical rhythm | `p SPACE_3XL` on all four sides (a zone pads uniformly, §6.1 — a header divider above it is a rule, not padding), `gap SPACE_XL` |
| `card(rows)` | A filled, hairline-bordered card whose rows are separated by full-bleed `divider_h` rules rather than by whitespace — the shape Settings' sections and the project-settings theme block both draw. Rows carry their own padding; the card contributes none, so a row's fill can reach the card's inner edge (see `RowDensity::Card`) | `RADIUS_CONTROL`, 1px `BORDER`, `BG_STRIP` |
| `footer_container(content)` | Full-bleed footer strip on `BG_STRIP`, bottom corners at `FOOTER_RADIUS` (11) | `px SPACE_3XL`, `py SPACE_LG` |
| `modal_footer(left, hints, buttons)` | **The** footer for every modal: left cluster = context/secondary/destructive action, a spacer, right cluster = hints then buttons. Hints sit immediately left of the buttons, or right-most when there are none — the spacer pushes the whole right cluster over, not per-caller alignment. An empty `hints` or empty `buttons` contributes no group at all, so there's no empty flex box eating a gap | `footer_container` + `gap SPACE_XL` between clusters |
| `modal_footer_hints(&[(key, label)])` | Footer of plain hints. Now a thin call to `modal_footer(None, hints, vec![])` — because `modal_footer` right-aligns its right cluster when there are no buttons, the hints render right-aligned, not left | `gap SPACE_3XL` |
| `modal_footer_row(content)` | `footer_container` for a fully custom row | — |
| `divider_h()` | Full-bleed 1px rule in `BORDER_SOFT` — sections *inside* a panel | `px(1.0)` |
| `divider_h_strong()` | The same rule at full `BORDER` strength, for *structural* zone edges (chrome bar boundaries) — §7.2's two tones | `px(1.0)` |
| `divider_h_toned(tone)` | The shared hairline shell behind both, for the rare rule neither name covers | `px(1.0)` |
| `vline()` | 1px × 16 vertical hairline in `BORDER`, separating clusters in a bar | `px(1.0)`, `h rpx(16)` |

#### Controls

| Component | Contract | Fixed tokens | Variants | States |
|---|---|---|---|---|
| `modal_action(id, label, kind, on_click)` | Footer/action button | `px SPACE_2XL`, `py SPACE_MD`, `RADIUS_CONTROL`, 1px border, sans `TEXT_BODY` | `ModalBtn::{Plain,Primary,Danger,Accent}` | default, hover (`BG_HOVER`), pointer cursor |
| `modal_action_sized(…, size, …)` | As above with an explicit text size | as above | + free text size | as above |
| `click_action(id, label, kind, dispatch, click)` | `modal_action` wired to a `ModalClick` | as above | as `ModalBtn` | as above |
| `click_action_enabled(id, label, kind, enabled, dispatch, click)` | `click_action` with an enabled axis, mirroring `click_checkbox`'s parameter order: when `enabled: false` it keeps the same box geometry — so the footer doesn't reflow when the action becomes available — but drops everything interactive: `BORDER_SOFT` border instead of the weight's, unfilled `BG`, `FG_MUTE` label, no `cursor_pointer`, no hover, no `on_mouse_down` at all (handler structurally absent per §10.1, never `opacity()`) | `px SPACE_2XL`, `py SPACE_MD`, `RADIUS_CONTROL`, 1px border | as `ModalBtn` | default, disabled (`enabled: false`) |
| `modal_checkbox(id, label, checked, accent, on_toggle)` | 14px box; `accent` colours tick + checked border. The tick is the `icons::icon("check", ..)` sprite, never a text-run character literal — see §9.3's mark rule | box 14, `RADIUS_CONTROL`, tick sprite sized to `CHECKBOX_TICK` (box − 2), `gap SPACE_LG`, label sans `TEXT_BODY` | `accent` | default, hover (`opacity 0.85`), checked, **disabled** (`on_toggle: None` → `FG_MUTE`, no pointer, no handler) |
| `click_checkbox(…, enabled, dispatch, click)` | `modal_checkbox` wired to a `ModalClick` | as above | as above | as above |
| `icon_btn(id, name, box_w, box_h, icon_size, color, hover_bg, hover_fg, hover_ring, on_click)` | **The** icon button. Every icon button in the app is this function | `RADIUS_CONTROL`, centred, `cursor_pointer` | box w/h, glyph size, rest tint, hover bg, optional hover fg, optional hover ring | default, hover (bg, optional fg recolour, optional `BORDER_SOFT` ring) |
| `flat_icon_btn(id, name, box_w, icon_size, on_click)` | Thin wrapper: `icon_btn` at `CONTROL_H`, `FG_DIM` rest, `BG_HOVER` hover, no ring | `h CONTROL_H` (22) | `box_w`, `icon_size` | default, hover |
| `flat_text_btn(id, label, text_size, h_padding, on_click)` | Flat borderless text button in the same 22px shape | `h CONTROL_H`, `RADIUS_CONTROL`, mono `FG_DIM` | text size, h-padding | default, hover |
| `flat_text_btn_tinted(id, label, text_size, h_padding, color, on_click)` | `flat_text_btn` with a colour axis, so a low-emphasis destructive action ("Archive project") has a component to be instead of a bare `ui()` run with a raw `on_mouse_down` — no button shape, no hover. A full `Danger` `modal_action` in that footer slot would compete with the footer's own Save/Primary button, which is why this stays flat rather than promoted to a weight | `h CONTROL_H`, `RADIUS_CONTROL` | text size, h-padding, `color` | default, hover |
| `field_underline(focused: bool) -> Div` | The app-wide borderless text field shell: a `CONTROL_H` box, mono text vertically centred, 1px bottom rule, `MAGENTA` when focused / `BORDER_SOFT` at rest. §8.1 makes every in-row control `CONTROL_H`; this was the one control that wasn't — a bare text run with a rule crammed under it, open-coded twice inside `scripts_editor` before the unification. Takes only `focused`; the caller chains `.child(...)` on the returned `Div` to build the field. **Caller contract:** the wrapped `Input` must zero gpui-component's own `input_px`/`input_py` and claim its width, verbatim `.appearance(false).pl(px(0.0)).pr(px(0.0)).py(px(0.0)).w_full()` — `Input` applies that padding regardless of `.appearance(false)`, which drops only the border and fill, so an unzeroed inset is what breaks a field's left edge out of true against the rest of the panel, and `w_full()` is what stops the field collapsing to its content inside this shell's `min_w_0` flex row. Also `overflow_hidden()`, so a long value clips at the field's own right edge instead of running into the enclosing card's border | `h CONTROL_H`, `w_full`, `overflow_hidden`, `border_b_1` | — | default (`BORDER_SOFT`), focused (`MAGENTA`) |
| `seg_button(id, label, active, side, danger, on_click)` | One segment of a two-way segmented control. Sized by `CONTROL_H` rather than by vertical padding, so a segment is the same 22px as every other in-row control (§8.1) — the same shape `flat_text_btn` uses; `py` would stack on top of the fixed height, so there is none | `h CONTROL_H`, `px SPACE_2XL`, mono `TEXT_SMALL` | `SegSide::{Left,Right}`, `danger` | default, hover, **active**, **inert** (`on_click: None`) |
| `seg_button_content(id, content, active, side, danger, on_click)` | `seg_button`'s shell around arbitrary content, for glyph segments; `content` owns its padding | outer corners at `RADIUS_CONTROL` on `side` only | as above | as above |
| `seg_text_color(active, danger)` | The label tint rule: active+danger → `RED`, active → `FG`, else `FG_DIM` | — | — | — |
| `seg_group(content)` | Bordered wrapper for a joined segment pair: 1px `BORDER`, `RADIUS_GROUP`, no internal gap | radius 6 | — | static |

`ModalBtn` weights (`src/views/components.rs` — `impl ModalBtn`):

| Weight | Text | Border | Fill | Use |
|---|---|---|---|---|
| `Plain` | `FG_DIM` | `BORDER` | `BG` (unfilled) | Dismiss / secondary |
| `Primary` | `FG` | `BORDER` | `BG_HL` | Default affirmative |
| `Danger` | `RED` | `RED` | `BG_HL` | Affirmative with destructive consequences |
| `Accent` | `FG` | `MAGENTA` α0.45 | `BG_HL` | Affirmative with emphasis; border goes to full `MAGENTA` on hover, applied inside the component per §9.1's hover restriction |

`Accent`'s rest alpha is the named constant `ACCENT_BORDER_REST_ALPHA` (0.45,
`src/views/components.rs`): the accent is present at rest but held back, so the
full-strength hover border reads as a *change*. It shares `MAGENTA`'s accent
role — the button that starts something new.

Active segment fill is `BG_HL`, or `RED_WASH` when `danger`
(`src/views/components.rs` — `seg_button_content`).

`SegSide` exists because only the group's **outer** corners round — the seam
between two segments must stay square or the group reads as two buttons.

`on_click: None` renders a segment inert, and it is used for the side that is
*already active*, so clicking it can never toggle the control back off. A
segmented control is a choice, not a switch.

**Why `icon_btn` takes so many parameters.** gpui's `hover` refuses to be
called twice on one element — gpui's `Div::hover` carries a `debug_assert!` to
that effect — so every hover axis has to be decided inside the component rather
than chained on afterwards (`src/views/components.rs` — `icon_btn`). That is the reason the hover treatment is a
parameter list and not a builder.

#### Chips, dots and rows

| Component | Contract | Fixed tokens | States |
|---|---|---|---|
| `keycap(inner)` | Keycap chip shell, `BG_HL` fill; `inner` carries its own colour | `px SPACE_MD`, `py SPACE_XS`, `RADIUS_CONTROL` | static |
| `keycap_filled(fill, inner)` | The same chip with the fill as an axis: `BG_HL` for a literal keycap, `BORDER_SOFT` for a neutral metadata chip (branch and count chips), an accent alpha for a live cue (the grid's number hint and respond chip). `keycap` is this at `BG_HL` | shape as `keycap` | static |
| `keycap_text(label, color)` | Plain-label keycap (`⏎`, `↑↓`, `esc`, `←→`) | mono `TEXT_SMALL` | static |
| `footer_hint(key, label)` | One keycap + muted label pair | `gap SPACE_MD`, label mono `TEXT_MICRO` `FG_MUTE` | static |
| `cue_chip(label)` | Palette leading-glyph cue when a drill-in replaces the search icon | `SEL_TINT_SOFT` fill, mono `TEXT_MICRO` `CYAN` | static |
| `status_dot(size, color)` | **The** filled activity dot — statusbar, terminal tab bar, sidebar rollups, session header | `rounded_full` | static |
| `status_dot_hollow(size, color)` | `status_dot`'s "absent / not installed" counterpart: same circle, same size, drawn as a 1px ring (`border_1`, `border_color`) instead of a filled `bg` — colour alone must never be the sole carrier of state (§2.3); filled versus hollow is a shape difference that survives greyscale, colour-blindness and a dimmed display, which is why it keeps the full 1px hairline in the state's own colour rather than a washed-out fill | `rounded_full`, `border_1` | static |
| `icon_slot(name, size, color)` | Fixed 24px icon slot so titles align regardless of glyph width | w 24 | static |
| `status_gutter(dot)` | A fixed `STATUS_DOT_COL_W` column, reserved as column one on **every** settings row whether or not that row carries a status dot. Labels then start at the same x whether or not their row shows a dot — `icon_slot`'s rationale ("a fixed slot so titles align regardless of glyph width") applied to the row grid rather than a new idea. Fixed at `CONTROL_H` tall and centres its mark *inside that height*, not inside the row's overall height — the row's outer container is `items_start` (so a tall sublabel never drags the whole row's cross-axis alignment around), which pins this gutter's top edge to the row's first line, and matching that line's own height is what puts the mark's centre on the label line. Centring on the row's overall height instead was a real shipped bug the user caught in the running app: with a sublabel present, the row's centre falls *between* the label and sublabel lines, so the dot floats above the label rather than sitting on it | w `STATUS_DOT_COL_W` (15), h `CONTROL_H` | static |
| `click_row(id, selected, density, dispatch, click, content)` | The clickable list row every list shares. `density` picks how much room the row gives its content — see `RowDensity` below | `gap SPACE_LG`, plus whatever `density` picks | `RowDensity::{Compact,Manager,Card}` | default, hover (`BG_HOVER`), selected (`BG_HL`), pointer |
| `palette_row(id, selected, dispatch, click, content)` | Palette results row | `h PALETTE_ROW_H` (54), `px SPACE_2XL`, `RADIUS_GROUP` | default, hover (`BG_HOVER`, **unselected only**), selected (`SEL_TINT_SOFT` + 1px `SEL_RING`), pointer |

`RowDensity` (`src/views/components.rs`) is the axis `click_row` was missing
when the theme manager's row used to fork the row shape — see §9.2, now down
to five forks because this one resolved:

| Variant | `px` | `py` | Corners | Fill |
|---|---|---|---|---|
| `Compact` | `SPACE_LG` | `SPACE_SM` | `RADIUS_CONTROL` | inset, rounded |
| `Manager` | `SPACE_XL` | `SPACE_MD` | `RADIUS_GROUP` | inset, rounded |
| `Card` | `SPACE_XL` | from content | square | full-bleed |

`Card` takes its height from its content rather than from padding, and its
hover fill is full-bleed and square: a rounded fill inset from the card's own
edge would read as a second, floating surface inside the card.

`SublabelTone` (`src/views/components.rs`) is `row_sublabel`'s two-tone axis:
`Normal` is `caption`'s `FG_MUTE`, `Safety` is one shade up — `caption_promoted`'s
`FG_DIM`, reserved for safety-relevant text (skip-permissions and friends).

#### Handler types

| Type | Contract |
|---|---|
| `OnToggle` | `Box<dyn Fn(&mut Window, &mut App)>` — a checkbox's toggle handler. Taken as `Option<OnToggle>`; `None` *is* the disabled state (§10.1), which is the `Option<handler>` idiom §14 mandates spelled as a name |
| `ModalDispatch` | `Rc<dyn Fn(ModalClick, &mut Window, &mut App)>` — see §3; components take it by reference and clone the `Rc` |

**`id` uniqueness is a hard contract.** gpui bleeds hover state between
duplicate ids (`src/views/components.rs` — `modal_action`). Every stateful component takes an `id`
and it must be unique within its view.

**Pointer cursor is uniform.** Anything clickable shows `cursor_pointer`; it
falls out of `icon_btn`, `modal_action_sized`, `click_row`, `palette_row`,
`flat_text_btn`, `seg_button_content` and the enabled `modal_checkbox`. A
clickable element without it is a bug.

### 9.1.1 Panel-modal grammar

Every panel modal in `src/views/modals/` — all 23 of them — shares one
grammar. It is written up here as a contract rather than as a list of
individual fixes, because a panel modal is not a pile of independent choices —
each rule exists because of what sits next to it.

App Settings and Project Settings are the origin story. The two were built
independently and had drifted into two different modal grammars — different
title sizes, a subtitle on one line versus two, a card label on one and a bare
heading on the other, a status dot leading one row family and trailing the
other. The Settings-modal unification that fixed those sixteen-plus drifts is
the contract below — and it did not stop at the two modals that surfaced it;
it was applied across every panel modal in the app.

**The four-child shape.** Every panel modal is, in order: header,
`divider_h()`, `modal_body`, footer. No panel modal skips a child or reorders
them, with exactly two exceptions, both pre-existing layout cases rather than
drift:

- **Onboarding** replaces the whole screen — full-viewport, no scrim, no panel
  shell, no close X — and uses a "Skip setup" affordance in place of a close
  button.
- **The command palette** top-drops (`scrim_top_drop`) and has a search row
  instead of a title row, so it has neither a header nor a close X. It is
  still bound by "the one rule after the header" below: it must not put a
  `divider_h()` directly above its footer, even though it has no header
  divider at the top to begin with.

**Header.** Always title (`TEXT_TITLE`) plus a close button, and a subtitle on
a second line when the modal has one to give. `MAGENTA` is the default accent
for every modal header, not only the settings family; `RED` is for a
destructive one ("Remove project", "Delete worktree"). `CYAN` and `AMBER` are
not modal-header colours — both were, before the sweep, and both are gone now:
one accent per emphasis level (default, destructive), not a header-specific
palette of its own. The subtitle is mono when it names a path or a value,
sans when it is prose; App Settings has none, because its one piece of
standing context is the save story, and that belongs in the footer (below).
Two lines, never one: a subtitle inline to the right of the title reads as
decoration the moment the title is long enough to push it out toward the
close button.

**Close button, with three named exceptions.** Every panel modal has a close
button, `flat_icon_btn` at the header icon tier (28 + `ICON_MD`) — a modal
dismissible only by `esc` fails the pointer user, and there is no reason a
keyboard-only exit should be the only one. Three states refuse or repurpose
`ModalClick::Cancel` closely enough that a close X would be dead or actively
misleading; each renders a plain `modal_header` (no close) and no footer at
all:

- `Teardown`/`RunningScript` (`src/views/modals/project.rs`) — Cancel means
  "skip the teardown script", not "dismiss the modal".
- `Teardown`/`Removing` and `RemoveProject`'s `in_progress` state
  (`src/views/modals/project.rs`) — Cancel is refused outright
  (`CancelOutcome::Refused`) while the destructive operation is running.
- `Updating`/`Updating` (`src/views/modals/settings.rs`,
  `UpgradeState::Updating`) — Escape/Cancel is refused while an update is in
  flight.

**The one rule after the header.** `divider_h()` sits directly under the
header, and nowhere above the footer. `footer_container`'s `BG_STRIP` fill is
already the visual edge of the footer zone — a lighter surface changing to a
darker one *is* a seam. Adding a rule on top of that fill would be two
separators marking the same boundary, and the header needs the rule because
the header has no fill change of its own to do the job.

**Sections.** Every section in a panel modal is a `settings_card_block`: a
mono, uppercase `TEXT_MICRO` label at `CARD_LABEL_INDENT` sitting over a
`card()`. A bare `card()` with no label, or a sans heading with no card at
all, are the two ways this used to fork — both are gone. Rows inside the card
are divider-separated by `card()`'s own hairlines; a row table that skips
those dividers is a bespoke shape next to a shared one.

**Rows.** One row family, on `setting_row_grid`, sized by the `ROW_PX`/`ROW_PY`
padding contract with `ROW_MIN_H` as a floor, not by a pinned per-kind height
(§8.1). `status_gutter` reserves its column on every row so labels
align whether or not a row shows a dot (§9.1). Disabled is `FG_MUTE()` plus a
dropped handler — `Option<handler>` structurally prevents the click, per
§10.1's disabled pattern — and it is **never** `opacity()`: opacity is not a
state in this system (§13), and a half-transparent row still paints a hover
and still eats a click unless the handler is actually gone. Explanatory text
under a row is a `row_sublabel`, never a `gpui_component` tooltip: a tooltip
is invisible to a keyboard user, and it duplicates a component
(`row_sublabel`) that already exists for exactly this job. Values are mono
`TEXT_BODY` `FG_DIM` and never take an accent tint — §2.3 already requires the
*text itself* to carry the state ("Default (follow app)" versus a theme name),
so a tint on top of that would be a second channel for a signal that only
needed one, and the first candidate for a third selection weight that §4.2
warns against inventing.

**Footer.** `modal_footer(left, hints, buttons)` is the one implementation
every modal's footer goes through. Left cluster = context and
secondary/destructive actions, a spacer, right cluster = hints then buttons.
The hint always sits immediately left of the button pair, or right-most when
there are none. A low-emphasis destructive action in the left cluster —
"Archive project" — is a `flat_text_btn_tinted` in `RED`, not a full
`ModalBtn::Danger`: a bordered danger button competing with the right
cluster's Save reads as two calls to action instead of one primary action with
an escape hatch beside it.

**One scroll cap.** `MODAL_SCROLL_MAX_H` (§6) is the scroll-viewport cap for
every scrolling modal body — Settings, Project Settings/ScriptsEditor, the
command palette results list, the theme manager list, and any modal body added
after this one. It is one constant, not a per-modal number to pick.

**One multiline exception.** The ThemeManager JSON editor is the one field in
the app that keeps a bordered box instead of `field_underline`, because
`field_underline` is `CONTROL_H` (22px) tall and physically cannot host a
multi-row text buffer. Every other single-line field in the app uses
`field_underline`.

**Selection lists.** Every selection list uses `card()` +
`click_row(RowDensity::Card)`. `palette_row` (`PALETTE_ROW_H`, 54px — §9.1) is
reserved specifically for the command palette, because its rows carry a title
*and* a subtitle, which `card()`/`click_row` rows don't — a second shape for a
genuinely second layout, not a fork of the first.

**The two save models stay different, on purpose.** App Settings persists
each control the moment it changes; Project Settings is a form with an
explicit Save. This looks like the same kind of inconsistency the rest of this
unification erased, but it is not — it is a real difference in what the modal
*is*. Unifying it would be worse in both directions: an Apply button bolted
onto a theme picker adds a step to a change that should be instant, and a
project rename that commits mid-keystroke would write a half-typed name to
disk the moment the field lost focus. Two save stories are the correct number
here; the rest of the grammar above is what makes them still read as one
family.

### 9.2 What stays deliberately local, and why

Consolidation is done; these five are the intentional exceptions, each for a
structural reason rather than inertia. Each names the **axis the shared
component is missing**, because that is the thing that would have to change for
the fork to end. (A sixth case — the theme manager's row — used to fork
`click_row` over row density; that axis now exists as `RowDensity`, §9.1, so
the row is no longer local and is not listed here.)

| Local shape | Where | Why it is not shared |
|---|---|---|
| `label_text` | `src/views/session_header.rs` — `label_text` | Wraps `components::ui` and adds an optional `FontWeight::SEMIBOLD`. `ui()` has no weight parameter, and adding one to the app-wide primitive to serve one bar is the wrong trade. |
| `hint_chip`'s key text | `src/views/statusbar.rs` — `hint_chip` | The chip's hover recolour lives on the parent row; a colour pinned on the inner run would win over the parent and kill the hover. |
| `agent_menu`'s item label | `src/views/sidebar.rs` — `agent_menu` | Same reason, stated in the code comment there: "the item's hover recolor lives on the row, so this label must inherit its color." |
| `row_actions`' agent bar | `src/views/modals/launcher.rs` — `row_actions` | Missing **three** axes at once: box size (`icon_slot` pins a private 24px `ICON_SLOT_W`, the bar needs `AGENT_BTN` = 26), corner rounding (`icon_slot` has none, the bar wants `RADIUS_GROUP`) and a selected-state 1px `YELLOW` border. `icon_slot`'s whole contract is *a fixed slot so titles align*; parameterising all three would leave nothing of it. |
| Empty-state panels | `src/views/grid.rs` — `empty_state`, `src/views/rows.rs` — `empty_row`, `src/views/terminal_tab.rs` — `empty_terminals_state` | Three genuinely different containers — a full-bleed `size_full` canvas, an inline `py(24)` block inside a scrolling list, and a keycap-carrying hint. Only their inner text is shared, and it already is: all three build from `components::ui` / `mono` at `TEXT_TITLE` + `TEXT_BODY`. |
| `add_project.rs`'s local `FIELD_H` (28) | `src/views/modals/add_project.rs` | Deliberately **not** on the §8.1 padding contract, and not a candidate to move onto it. Its directory-picker list is windowed by `crate::launcher::scroll_offset_for`, whose visible-slice arithmetic assumes a fixed pixel row height — a content-sized row would desync the picker's scroll math from what is actually on screen. |

The general rule this encodes: **share the text and the shell, not the
colour-inheritance decision.** Whenever a parent owns the hover recolour, the
child must stay colourless.

### 9.3 Icons — `src/icons.rs`

An in-memory SVG sprite. Nothing ships as a file: `Assets::load` answers
`icons/<name>.svg` and `icons/spinner-<frame>.svg` straight out of a
`&str -> String` table.

- `icon(name, size, color) -> Svg` — square, tinted, sized with `rpx`.
- `spinner(size, color, tick) -> Svg` — 12 pre-rotated frames, advancing one
  step every 3 clock ticks.

Colour is **not** baked into the SVG text: paths use `stroke="currentColor"` /
`fill="currentColor"` and gpui's `Svg` paints with `style.text.color`, so the
tint is applied via `Styled::text_color`. `Svg` has no `color` method at this
rev (`src/icons.rs` — module doc).

All sprites are authored on a 16×16 `viewBox` with `stroke-width: 1.6`, round
caps and joins (`src/icons.rs` — `wrap_svg`). Fill-based marks (the agent
brand glyphs, `cog`, `command`, `sparkle`) opt out with
`fill="currentColor" stroke="none"`.

An unknown name degrades to an empty `<svg>` rather than panicking — a missing
icon must never take down a window.

**A pictographic mark comes from the sprite, never from a text-run character
literal.** `modal_checkbox`'s tick used to be a literal `"✓"` (U+2713) inside a
`mono()`/`ui()` run; the bundled fonts (`fonts::UI_FAMILY`, `fonts::MONO_FAMILY`)
have no glyph coverage for that codepoint, or for the wider Dingbats
(U+2700–U+27BF), Miscellaneous Symbols (U+2600–U+26FF), Geometric Shapes
(U+25A0–U+25FF), Box Drawing (U+2500–U+257F) or Braille Patterns (U+2800–U+28FF)
blocks, so the literal silently fell back to a stand-in glyph rather than
rendering the mark. The fix (`components::modal_checkbox`) draws the tick with
`icons::icon("check", ..)` instead. Any new pictographic mark — a checkmark,
cross, filled/hollow dot, star and the like — belongs in `src/icons.rs`'s
sprite table, not as a character literal in a `ui()`/`mono()` string.

This does **not** ban the keycap pattern in §5.2 (`⏎`, `↑↓`, `esc`, `←→`):
those are real keyboard-key characters, deliberately rendered as text because
they are read as *keys*, not as marks standing in for an icon, and the bundled
fonts do cover them. The rule is about marks that are standing in for a sprite
glyph the fonts do not have, not about banning any character outside ASCII.

---

## 10. States

### 10.1 Interaction states

This is the real vocabulary in the tree, not an aspirational one.

| State | Treatment | Coverage |
|---|---|---|
| **Default** | Rest fill, `FG_DIM` (or `FG_MUTE`) label/icon | universal |
| **Hover** | `BG_HOVER` fill, optionally a `text_color` recolour or a `BORDER_SOFT` ring; `opacity(0.85)` for the checkbox row. Always with `cursor_pointer` | universal |
| **Active / selected** | `BG_HL` (neutral), `SEL_TINT_SOFT` + `SEL_RING` (selection), `SEL_TINT_STRONG` + `SEL_RING` (edit/rename), `RED_WASH` (danger segment) | universal where applicable |
| **Inert** | An already-active segment takes `on_click: None` — no hover, no pointer, no handler | `seg_button` / `seg_button_content` |
| **Disabled** | `FG_MUTE` text and glyph, no pointer, no hover, no handler attached | `modal_checkbox` / `click_checkbox` only (§15) |

**There is no focus-visible treatment, by decision.** Grove does not render
focus rings, and there is no `FOCUS_RING` token. `FocusHandle` exists where
keyboard routing needs it, but keyboard position is communicated by the
*selection* treatment — the list's own tint-plus-ring — not by a separate focus
affordance. A cyan ring would collide with the selection language it sits next
to. **Do not add a one-off focus outline**, and do not describe the system as
having one.

The disabled pattern is worth naming because it is unusually clean: passing
`None` for the handler *structurally* prevents interactivity, rather than
relying on a boolean a call site can forget to check (`src/views/components.rs`
— `modal_checkbox`, `seg_button_content`).

### 10.2 Data states

| State | Meaning | Pattern in the tree |
|---|---|---|
| **Loading** | Work in flight | `icons::spinner` at `ICON_SM` in `GREEN` — the `Working` state glyph (`src/views/rows.rs` — `state_glyph`) |
| **Empty — never had** | Nothing created yet | An *invitation*: "No projects yet / Add one with + above" (`rows::sidebar_empty_copy`) |
| **Empty — no match / nothing selected** | Filtered or unselected to zero | `grid::empty_state(title, subtitle)` (`src/views/grid.rs`) |
| **Partial** | Some data present, some pending | Row-level: `state_glyph` renders per-row state independently, so a tree can be mixed without a global spinner |
| **Error** | The operation failed | `note_text` for inline validation; `ActivityState::Exited` → `ring` glyph in `FG_MUTE` |

The empty-state distinction is load-bearing. `sidebar_empty_copy` branches on
how the user got to zero: nothing-yet earns an invitation, whereas
everything-archived earns a *recovery path*. A single "nothing here" string for
both strands the second user.

**Every new surface owes all five.** If a state is genuinely impossible for a
surface, that is a claim you should be able to defend, not an omission.

### 10.3 The activity-state vocabulary

`rows::state_glyph` (`src/views/rows.rs`) is the canonical mapping. Every glyph
is drawn at `ICON_SM` inside a **fixed 14px slot** (`src/views/rows.rs` —
`GLYPH_SLOT_W`) whichever state is active, so nothing reflows (§2.4).

| State | Glyph | Colour | Extra channel |
|---|---|---|---|
| `Working` | spinner | `GREEN` | rotation |
| `WaitingForInput` | `question` | `AMBER` | alpha pulse `1.0 − 0.45·pulse`; **dims, never hides** |
| `Done` | `check` | `FG_MUTE` | — |
| `Idle` | `dot` | `FG_MUTE` | — |
| `Exited` | `ring` | `FG_MUTE` | — |

The three quiet states share `FG_MUTE` and are distinguished **entirely by
glyph shape**. That is deliberate: the colour channel is reserved for the two
states that want your attention.

---

## 11. Motion

### 11.1 The clock

One monotonic counter drives **every** blink phase in the app, so the phase
relationship between the cursor, the dots and the spinner is exact
(`src/entities/animation_clock.rs` — module doc).

| Lane | Period | When |
|---|---|---|
| `FAST` | 60ms | `busy \|\| (has_ptys && (focused \|\| animating \|\| dirty))` |
| `SLOW` | 1s | otherwise |

The gating predicate is `is_fast` (`src/entities/animation_clock.rs`). Getting it wrong
is an idle-power regression, not a cosmetic bug.

### 11.2 Derived phases

All pure functions of the tick:

| Phase | Formula | Result |
|---|---|---|
| Cursor blink | `tick % 16 < 8` | 960ms period, 480 on / 480 off |
| Thinking dots | `(tick / 5) % 3` | which of three dots is lit |
| Spinner frame | `(tick / 3) % 12` | 12 pre-rotated arc frames |
| Toast pulse | `tick % 40` | 2.4s pulse period |

The cursor formula is a parity contract with the iced build. **Never re-derive
it** from the 533ms figure quoted in the older sources
(`src/entities/animation_clock.rs` — `cursor_visible`).

### 11.3 Triangle waves on alpha only

Pulses are triangle waves applied to **alpha**, never to size, position or
layout:

```rust
appbar::pill_dot_alpha(pulse) = 1.0 - 0.4  * pulse   // attention pill dot
grid::respond_alpha(pulse)    = 1.0 - 0.35 * pulse   // "respond" chip
grid::scrim_alpha(pulse)      = 0.7 + 0.3  * pulse   // tile scrim
rows::state_glyph waiting     = 1.0 - 0.45 * pulse   // attention question mark
```

The reason is written into the code: "layout cannot shift as it pulses"
(`src/views/appbar.rs` — `pill_dot_alpha`). Animating a dimension would reflow the row sixteen times a
second.

Not everything is clock-derived: the attention amber pulse (1s auto-reverse
EaseInOut) and the onboarding entrance use gpui's `with_animation`. **Do not
wire those to the tick** (`src/entities/animation_clock.rs` — module doc).

---

## 12. Accessibility

**Colour is never the sole carrier of state.** Every signal pairs colour with a
second channel:

| Signal | Colour | Second channel |
|---|---|---|
| Running | `GREEN` dot | numeric count beside it (`src/views/statusbar.rs` — `statusbar`) |
| Working | `GREEN` | rotating spinner glyph |
| Needs you | `AMBER` | `question` glyph + a pulse |
| Done | `FG_MUTE` | `check` glyph |
| Idle | `FG_MUTE` | `dot` glyph |
| Exited | `FG_MUTE` | `ring` glyph |
| Destructive | `RED` | the word ("Delete", "Archive") + `ModalBtn::Danger` border |
| Selected | cyan tint | a 1px ring, plus the row's fill |
| Edit/rename | stronger cyan tint | the same ring plus an inline editor |

**Layout must not move when state changes** (§2.4, §10.3).

**Keyboard reachability.** Every common action has a key path, surfaced through
the statusbar hint chips, the modal footer hints (`footer_hint`) and the grid's
numeric chord hints. On macOS the modifier renders as the `command` glyph;
elsewhere as `"{mod}+{key}"` text (`src/views/statusbar.rs` — `hint_chip`). Keyboard position
is shown by the selection treatment, not a focus ring (§10.1).

**Contrast** is a known limitation, not a guarantee — see §15.

---

## 13. Anti-patterns

**Visual**

- Marketing hero layouts inside the app.
- Decorative gradients, glow, glass, status-card dashboards.
- Display type or display glyphs (`TEXT_DISPLAY`, `TEXT_DISPLAY_LG`,
  `ICON_DISPLAY`) anywhere in chrome.
- Decorative italics.
- Nested cards. One level deep, for repeated items, modals and framed tool
  surfaces.
- Chrome that competes with PTY output.
- `opacity()` as a disabled state. Disabled is `FG_MUTE()` plus a dropped
  handler (§10.1, §9.1.1) — a dimmed-but-live control still paints a hover and
  still eats a click.
- A tooltip standing in for a sublabel. A tooltip is invisible to a keyboard
  user; `row_sublabel` exists for exactly this and does not have that problem
  (§9.1.1).
- Mono used for prose. Mono is for values and keys — a token read, not
  language (§5.2).
- An accent tint on a value. §2.3 already requires the text itself to carry
  the state; a tint on top is a second channel for a signal that needed one.
- A fifth type tier in chrome. Four tiers is the whole vocabulary (§5.3); a
  design that needs a fifth is a design that is wrong, not a gap in the scale.
- A bespoke row shape beside a shared row grid. One row family per surface,
  not a table that reinvents dividers, padding and a label column from
  scratch (§9.1.1).
- A pinned height on a content row. Padding sizes a row; a fixed height only
  fits the content the author happened to test, and either rattles around
  shorter content or clips taller content the moment it appears (§8.1).
- A fixed-size column whose mark aligns to the container rather than to the
  line it annotates. `status_gutter` centring on a row's overall height
  instead of its first line put the dot between the label and sublabel lines
  rather than on the label it names (§9.1).
- A field with no trailing inset. A value with padding on only one edge runs
  into the enclosing card's border the moment it is long enough (§9.1).

**Layout**

- Reflowing row heights when hover actions appear. Reserve the slot.
- Animating any dimension. Animate alpha.
- Sidebar width that jumps between renders.

**Code**

- A bare numeric literal in a styling call. Use a token.
- `rpx()` on a 1px hairline, on window/viewport math, or on the terminal grid.
- Blending colours in HSL space.
- `px()` for anything that should zoom.
- Naming a colour in a component. Name a role. (The palette preview's swatch
  strip is the one sanctioned exception — §4.4.)
- Tinting a token with `Hsla { a: .., ..c::TOKEN() }`. Call `c::alpha` (§4.3).
- A bare modal width. Take a `MODAL_W_*` notch (§8.5).
- An icon or dot size that is not on the `ICON_*` / `DOT_*` scale (§5.3.1).
- Reusing an `id` between two stateful elements in one view — gpui bleeds hover
  state between them.
- `window.line_height()` or `window.rem_size()` for the terminal grid.
- Wiring the attention/onboarding animations to the animation clock.
- Re-implementing an icon button, a status dot, a keycap, a text run, a
  segmented control or a hairline. They all exist in `components.rs`.
- Pinning a text colour on a run whose parent row owns the hover recolour.
- Adding a focus outline (§10.1).
- A derived constant left hardcoded after its input changed. `MODAL_SCROLL_MAX_H`
  as a bare pixel number would silently clip the first Tools row out of the
  scrollable body the next time row geometry moved; the fix is the formula
  (`ROW_MIN_H * 12`), not a new literal (§8.1).

**Behaviour**

- Treating embedded terminals as screenshots instead of live, selectable PTYs.
- A segmented control whose active side can be clicked back off.
- One "nothing here" empty state where the user's situation differs (§10.2).

---

## 14. Extending the system

### Adding a numeric token (`src/views/tokens.rs`)

1. Check whether an existing token is within a pixel or two. It almost always
   is — take the existing one.
2. If it genuinely is not, add it *in scale order*, with a doc comment naming
   its role and which tokens it sits between.
3. **Never add a token with exactly one consumer.** That is a module constant
   (`CHECKBOX_BOX`, `ICON_SLOT_W` and `AGENT_BTN` are the pattern).
4. A number belongs in `tokens.rs` only if it is **relational** — a notch on a
   scale with neighbours. Positional, singular geometry stays in its owning
   module (§8.4).

### Adding a colour token (`src/theme.rs`)

Adding one changes all ~40 themes at once.

1. Derive from tier-1 roles via `mix`/`alpha` on `Rgba`. Never a literal.
2. Branch on `is_dark_of(t)` if light themes need a different ratio — most
   surface derivations do.
3. Add the `_of(theme)` variant if PTY content could ever need it.
4. Add a test if it participates in an invariant (ordering, interval
   membership) — `chrome_surfaces_get_progressively_darker` and
   `amber_sits_between_yellow_and_red` are the patterns.
5. Document its role in §4.

**Layout geometry is not a token.** It goes in the owning module as a
`pub const` with a doc comment (§8.4).

### Adding a component

1. **Check §9.1 first.** If the shape exists, consume it; if it exists but is
   missing one axis, add the axis as a parameter rather than forking.
2. Pure function of plain data. Take an `id`, take a handler or dispatch,
   return an element. No entity, no `Context`, no state.
3. Express disabled/inert as `Option<handler>`, not a boolean.
4. Fixed tokens go inside the component; the variant axis goes in the
   signature. `modal_action` takes a `ModalBtn`, not a colour.
5. Implement every applicable state from §10.1 *inside* the component. A call
   site should never have to add a hover — and with gpui it often cannot, since
   `hover` may only be applied once per element.
6. Add a row to the §9.1 contract table.

### When a bare literal is legitimate

Four cases, and only four:

1. **1px hairlines** — `px(1.0)`. A hairline is not on a scale.
2. **Derived geometry** — a value computed *from* a token, expressed as a named
   constant: `FOOTER_RADIUS = RADIUS_PANEL - 1` (§7.3).
3. **Optical corrections** — the modal panel's shadow, the palette scrim's 80px
   top drop, the appbar cog's 28px box and 15px glyph (at `CONTROL_H` its hover
   target clips against the window edge). Each one carries a comment saying
   *why*.
4. **Physical device-space math** — window bounds, mouse positions, PTY cell
   metrics.

Everything else is a token. If you find yourself writing a fifth category, add
it to §15 instead of to the code.

### Renaming a semantic token is a breaking change

`c::AMBER()` does not mean "amber", it means **needs-you**. Renaming it, or
repointing it at a different derivation, silently changes the meaning of every
call site — and every call site still compiles. Treat a rename as an API break:
audit every consumer and update §4 in the same commit.

The same applies to `ModalBtn::Danger`, the `ActivityState` glyph mapping, the
two selection weights, and the surface luminance ordering. These are the
vocabulary; changing what a word means costs more than adding a word.

---

## 15. Known limitations

Accurate as of this branch. Do not describe the system as covering these.

1. **No programmatic contrast assertion across the bundled themes.** ~40 themes
   ship, all derived at runtime from eleven roles. The derivations are built to
   track a theme's own contrast direction — `BORDER` is `mix(bg, fg, 0.16)`, not
   an assumption about dark-on-light — but the only mechanical guarantee is the
   three-surface luminance ordering test (§4.2). Nothing verifies that
   `FG_MUTE` on `BG_STRIP` is readable in every theme, and that is the
   most-used low-contrast pairing in the app. This is an accepted limitation:
   WCAG AA on 11px mono would fail several bundled themes outright, so there is
   no threshold to gate on today.

2. **Disabled is only partially systematised.** `modal_checkbox` /
   `click_checkbox` take `Option<handler>` and render a true disabled state;
   `seg_button` takes `Option` but renders *inert* rather than visibly
   disabled, which is the correct behaviour for its one use. `modal_action`,
   `icon_btn`, `flat_icon_btn`, `flat_text_btn`, `click_row` and `palette_row`
   have no disabled state at all — a call site that needs one has to not render
   the control.

3. **The display tiers have a narrow consumer set.** `TEXT_DISPLAY` (20),
   `TEXT_DISPLAY_LG` (32) and `ICON_DISPLAY` (32) exist for full-viewport empty
   and onboarding states. They are correct where used and must not spread into
   chrome.

4. **`#![allow(dead_code)]` is crate-wide practice, not a `components.rs`
   quirk.** It is on `components.rs`, `tokens.rs`, `theme.rs`, `zoom.rs`,
   `animation_clock.rs` and roughly twenty other modules on this branch. Some
   contracts and tokens are built ahead of their consumers (`bg_hl_of` and
   `sel_ring_of` in §4.4 are the current examples). The consequence is that the
   compiler will not tell you a helper is unused — a public item with no call
   site is not evidence a shape is dead, and grep is the only check before
   deleting.

5. **A retired conformance exception.** `src/views/conformance.rs`'s R4 check
   (§5.3) carried a `settings.rs` allow-list entry narrowed to `TEXT_DISPLAY`,
   for Project Settings' header when the project name itself was the modal's
   one title. The Settings-modal unification moved that header onto the
   ordinary `TEXT_TITLE` contract every other modal header uses (§9.1.1), so
   the exception no longer has a call site and is deleted along with it. It is
   worth recording that it existed: a rule that once had a reviewed, narrowly
   scoped exception is a stronger signal than a rule that was never tested,
   and the next time someone reaches for a fifth type tier this is the
   precedent that says it was tried and undone.
