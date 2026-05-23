---
name: Grove
description: Terminal worktree launchpad for AI coding agents — a TUI that themes itself out of the user's way.
colors:
  bg: "#1a1b26"
  bg-highlight: "#292e42"
  fg: "#c0caf5"
  fg-dark: "#a9b1d6"
  comment: "#565f89"
  blue: "#7aa2f7"
  cyan: "#7dcfff"
  magenta: "#bb9af7"
  green: "#9ece6a"
  yellow: "#e0af68"
  red: "#f7768e"
typography:
  body:
    fontFamily: "user terminal monospace (no font is bundled)"
    fontSize: "1ch × 1 row (terminal cell)"
    fontWeight: 400
    lineHeight: 1
    letterSpacing: "normal"
  emphasis:
    fontFamily: "user terminal monospace"
    fontWeight: 700
    fontFeature: "BOLD attribute"
rounded:
  none: "0"
spacing:
  cell: "1ch"
  gutter: "1 row"
components:
  pane-active:
    backgroundColor: "{colors.bg}"
    textColor: "{colors.fg}"
    rounded: "{rounded.none}"
  pane-inactive:
    backgroundColor: "{colors.bg}"
    textColor: "{colors.fg-dark}"
  selection-row:
    backgroundColor: "{colors.bg-highlight}"
    textColor: "{colors.fg}"
  status-running:
    textColor: "{colors.green}"
  status-error:
    textColor: "{colors.red}"
  hint-key:
    textColor: "{colors.yellow}"
  hint-label:
    textColor: "{colors.comment}"
---

# Design System: Grove

## 1. Overview

**Creative North Star: "The Borrowed Theme."**

Grove is a TUI. Its visible surface is whatever the user's terminal renders, in whatever theme the user picks from a roster of 36 (22 dark, 14 light). The design system is not a palette — it is a **semantic role map** that any of those 36 palettes can satisfy. The job of the design system is to make sure every screen still reads correctly whether the user is in `tokyonight`, `gruvbox-light`, `matrix`, or `solarized-light`.

This rejects three reflexes outright: the Electron-terminal aesthetic (cards, shadows, gradients in a buffer that has none of those primitives), the AI-tool dashboard reflex (hero metrics, status pills, "thinking…" shimmer), and the over-decorated CLI reflex (ASCII art banners, fancy box-drawing, color used for ornament). Grove's surface is two panes, a status line, and a footer hint. Everything else is content.

Density is high and uniform. The terminal cell is the spacing primitive; padding inside panes is one cell; gutters between sections are one row. There is no concept of "elevation"; depth is signaled by **focus** (which pane is bordered as active), not by shadow or layering.

**Key Characteristics:**
- Eight semantic color slots (`bg`, `bg_highlight`, `fg`, `fg_dark`, `comment`, `blue`, `cyan`, `magenta`, `green`, `yellow`, `red`) mapped to 36 concrete themes.
- Layout is always two-pane horizontal (30/70) with a 3-row banner, 1-row status, 1-row footer.
- No animation. No icons beyond a single Unicode dot `●`. No box-drawing beyond ratatui's default rounded borders.
- Focus is the only state worth painting; everything else stays in the default foreground.

## 2. Colors: The Semantic Eleven

Grove never owns absolute color values. It owns **roles**. Each role has one job; the user's chosen theme supplies the hex.

The example values throughout this document come from `tokyonight` (the default), and the same role lookups produce coherent results across every other theme because each theme is hand-tuned to satisfy the same semantic contract.

### Surface
- **`bg`** (`#1a1b26` in tokyonight): the canvas. Every pane, modal, and gutter sits on this. Never apply text directly to "no background" — the canvas is always painted.
- **`bg_highlight`** (`#292e42`): selection. The current row in any list, the focused tab, the highlighted theme preview. This is the *only* background variation in the system.

### Text
- **`fg`** (`#c0caf5`): primary text. Body content in the focused pane.
- **`fg_dark`** (`#a9b1d6`): secondary text. Body content in unfocused panes; metadata that supports a primary value.
- **`comment`** (`#565f89`): tertiary text. Footer hint labels, placeholder text, "no items" states, separator characters.

### Accent (used sparingly, by role)
- **`blue`** (`#7aa2f7`): identifiers — project names, worktree paths, headings that name a thing.
- **`cyan`** (`#7dcfff`): the active agent / current selection emphasis. Used to draw the eye when blue would compete.
- **`magenta`** (`#bb9af7`): meta-actions — modal titles, pickers, "choose an agent" affordances.
- **`green`** (`#9ece6a`): running. The `●` next to a project or worktree with a live session. Success messages.
- **`yellow`** (`#e0af68`): keybinding letters in footer hints (`q quit`, `?  help`). Always paired with a `comment`-colored label.
- **`red`** (`#f7768e`): errors and destructive prompts only. Never decorative.

### Named Rules

**The Eleven-Slot Rule.** A new piece of UI must map to one of these eleven roles. If a designer feels they need a twelfth color, they are decorating, not informing. Rewrite using the existing eleven.

**The Theme-Agnostic Rule.** No screenshot, mock, or component description in this codebase may reference a hex value. All references are by role name (`fg_dark`, `green`). The theme module is the single binding layer between roles and concrete RGB.

**The Yellow-for-Keys Rule.** Every keybinding letter shown to the user is `yellow`. Every label paired with it is `comment`. If you see a hint rendered without this pairing, it is a bug.

## 3. Typography

**Body Font:** the user's terminal font. Grove ships no fonts and assumes nothing beyond a monospace cell. All sizing is in terminal cells (1ch × 1 row).

**Character:** typographic hierarchy in a TUI is degenerate: there is no scale axis, no leading, no tracking. Hierarchy is carried by **color role and weight**, not size.

### Hierarchy
- **Banner title** (BOLD, `magenta` or `blue` per context): top-row identifier for what the user is looking at. One line. No subtitle.
- **List item — selected** (default weight, `fg` on `bg_highlight`): the cursor row in any pane.
- **List item — default** (default weight, `fg` in focused pane / `fg_dark` in unfocused): rows in projects/worktrees/sessions lists.
- **Metadata** (default weight, `comment`): paths, counts, "(no sessions)" placeholders.
- **Status line** (default weight, mixed roles): one row, left-aligned, semantic colors per segment.
- **Footer hint** (default weight, `yellow` key + `comment` label): single row, space-separated `key label` pairs.

### Named Rules

**The One-Row Rule.** Banner, status, and footer are each exactly the height ratatui's layout assigns them: 3, 1, 1. They do not grow. If a message will not fit, abbreviate; do not wrap.

**The No-Wrap-In-Chrome Rule.** Wrapping is permitted in body content (modal text, help). It is forbidden in the banner, status line, and footer. Those surfaces truncate.

## 4. Elevation

Grove has no elevation. There are no shadows, no gradients, no semi-transparent overlays. The terminal does not render them; pretending it does is a category error.

Depth is signaled by **focus and selection only**:
- The focused pane gets a border of `BorderType::Rounded` rendered in `fg` (or an accent role per context). Unfocused panes get the same border in `comment` or `fg_dark`.
- The selected row in a list gets `bg_highlight` as its background. That is the entire "lifted" vocabulary.

### Named Rules

**The Flat-Forever Rule.** No component in Grove may simulate elevation through Unicode block characters, repeated borders, or chained box-drawing. If a designer wants to imply "this is on top," they use a modal (a fully cleared Rect over the canvas), not a shadow.

## 5. Components

### Panes
- **Shape:** rectangular, `Block::default()` with `Borders::ALL` and `BorderType::Rounded`.
- **Active:** border in `fg` or a context accent (`blue` for the projects pane, `cyan` for sessions, etc.). Title rendered inline in the top border.
- **Inactive:** border in `comment`. Title in `fg_dark`.
- **Internal padding:** one cell. Content starts at `(x+1, y+1)` of the pane rect.

### Lists (projects, worktrees, sessions)
- **Row layout:** `<icon-cell> <primary> <metadata...>`. Primary is `fg`, metadata is `comment` separated by two spaces.
- **Selected row:** background = `bg_highlight`, foreground unchanged.
- **Running indicator:** a green `●` prefix followed by the session count, e.g. `●3`. The dot and count share the `green` role.
- **Empty state:** a single `comment`-colored line: `(no projects)`, `(no worktrees)`, `(no sessions)`. Never a button, never a call to action — the keybinding footer already advertises `a`.

### Modals
- **Trigger:** input prompts (add project, new worktree name), pickers (agent picker, theme picker).
- **Shape:** centered Rect, cleared with `Clear`, then drawn with the same rounded-border pane treatment. Title in `magenta`.
- **Body:** vertical stack, each item one row, selected item painted with `bg_highlight`. For input prompts: a single-line input field with a `cyan` cursor block.
- **Dismissal:** `esc` or `enter`. Never tap-outside; the terminal has no "outside."
- **Hint separator:** one empty row separates the last line of body content from the hint row. This is the only permitted inter-row gap inside a modal; it signals "content above / actions below" without resorting to a box-drawing divider.

### Status line
- **Position:** second-to-last row, height 1.
- **Style:** space-separated segments, each `comment label: fg value` or `role glyph + value` for state segments. Pane focus indicators live here.

### Footer
- **Position:** last row, height 1.
- **Style:** `yellow` key + space + `comment` label, repeated, two-space-separated, e.g. `j move  a add  d delete  r refresh  ? help  q quit`.
- **Rule:** every keystroke that is valid in the current screen must appear in the footer for that screen. If a binding is not in the footer, it does not exist.

### The session count indicator (signature)
The `●N` glyph appearing next to a project name or worktree row is Grove's only purely visual signal. It is `green`, always paired with a digit, never larger than what fits in one cell plus one digit. It collapses the answer to "is anything running here?" into one terminal cell, which is the entire point.

## 6. Do's and Don'ts

### Do:
- **Do** map every new UI surface to one of the eleven semantic color roles before writing any color into the source.
- **Do** signal focus with border color and title weight; signal selection with `bg_highlight`. Nothing else.
- **Do** keep banner / status / footer at their fixed heights (3 / 1 / 1). Truncate content; never grow chrome.
- **Do** pair every keybinding letter with `yellow` and every label with `comment` in footer hints.
- **Do** leave exactly one blank row between modal body content and the modal's hint row.
- **Do** use `●` (U+25CF) and only `●` as a state glyph. Pair it with a numeric count.
- **Do** test every change against `tokyonight`, one warm dark (`gruvbox`), one cool dark (`nord`), one light (`github-light`), and `matrix` — the five themes that break things first.

### Don't:
- **Don't** introduce a twelfth color slot. The eleven exist precisely because eight ANSI colors plus three neutrals is enough; a twelfth means you are decorating.
- **Don't** hardcode hex values anywhere outside `src/theme.rs`. The role lookup is the only contract.
- **Don't** simulate cards, shadows, or elevation with stacked box-drawing characters or repeated borders. The terminal is flat by physics.
- **Don't** add icons beyond `●`. No Nerd Font glyphs, no emoji in menus, no Unicode arrows in chrome.
- **Don't** animate. No spinners, no progress bars, no scrolling banners. Render the new state; let the terminal flip.
- **Don't** wrap text in the banner, status line, or footer. Truncate with no ellipsis if necessary.
- **Don't** prompt-question-prompt-question. Use a single modal with all needed inputs, or two keybindings for two actions.
- **Don't** ship a screen whose keybindings are not all visible in its footer. Hidden bindings are a contract violation.
- **Don't** assume dark mode. Light themes are first-class; 14 of them ship.
- **Don't** use `red` for anything except errors and destructive confirmations. It is the loudest role in every theme by design.
