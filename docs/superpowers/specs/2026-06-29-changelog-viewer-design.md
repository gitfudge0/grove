# Changelog Viewer — Design

## Goal

Let the user read the project's recent release notes from inside Grove. A
"View changelog" button under the Settings → Updates section opens a full
dedicated screen listing the ~10 most recent GitHub releases (tag, name, date,
and lightly-cleaned notes). This builds directly on the self-update feature,
which already talks to the GitHub releases API.

## Module boundary

All new data logic lives in the existing iced-free `src/upgrade.rs` module
(no `iced` dependency, unit-testable standalone), consistent with `latest()`
and `apply()`:

- **`releases(limit) -> Result<Vec<ReleaseNote>>`** — query the GitHub
  *list* releases endpoint and return up to `limit` recent entries.
- **`clean_markdown(&str) -> String`** — pure, light Markdown cleanup for
  display.

The GUI layer (`gui/state.rs`, `gui/update.rs`, `gui/view.rs`) orchestrates:
fetch off-thread, hold result in state, render the screen.

## New dependencies

None. Reuses `ureq` (rustls) and `semver` already added for self-update. No
date crate, no Markdown crate — see below.

## Data

A new struct in `upgrade.rs`:

```rust
pub struct ReleaseNote {
    pub tag: String,   // tag_name, e.g. "v0.25.0"
    pub name: String,  // release "name" (falls back to tag if empty/null)
    pub date: String,  // YYYY-MM-DD — first 10 chars of published_at ("" if absent)
    pub body: String,  // raw release notes (Markdown), cleaned at display time
}
```

`date` deliberately avoids a date-parsing crate: GitHub's `published_at` is
ISO 8601 (`2026-06-29T12:00:00Z`), so the first 10 characters are exactly the
`YYYY-MM-DD` prefix. If `published_at` is missing or shorter than 10 chars,
`date` is the empty string and the UI omits it.

## Fetch

`releases(limit)`:

1. `GET https://api.github.com/repos/gitfudge0/grove/releases?per_page={limit}`
   using a timeout-bounded `ureq` agent (connect 10s, read 30s) with the same
   `User-Agent: grove` and `Accept: application/vnd.github+json` headers as
   `latest()`. `limit` is passed both as `per_page` (so GitHub returns at most
   that many) and as a defensive `.take(limit)` after parsing.
2. The list endpoint **includes prereleases**. That is acceptable here — this
   is a human-readable history, not the update-offer path (which stays on
   `/latest`). No filtering is applied; whatever GitHub returns, newest first,
   is shown.
3. Parse the JSON array into `Vec<ReleaseNote>`. Each element: `tag_name`
   (required — skip any element missing it rather than failing the whole
   list), `name` (fall back to `tag` when null/empty), `published_at` (→ date
   prefix), `body` (default `""`).

Network/parse failure is non-fatal: it surfaces as an inline error state on
the changelog screen (this is a user-initiated action, so the error is shown,
mirroring the *manual* update check).

## Markdown cleanup

`clean_markdown(&str) -> String` — a small, pure, hand-rolled formatter (no
crate). Per line:

- Strip leading ATX heading markers: a run of `#` followed by a space at the
  start of a line is removed (`## Features` → `Features`).
- Normalize unordered-list markers: a leading `-`, `*`, or `+` followed by a
  space becomes `• ` (preserving any indentation before the marker).
- Trim trailing whitespace from every line.
- Collapse runs of 2+ blank lines into a single blank line.

It does **not** attempt full Markdown rendering (no bold/italic/link parsing,
no tables) — just enough to make release notes read cleanly as plain text in a
monospace/UI font. Inline markup like `**bold**` or `` `code` `` is left
as-is.

## State & messages (gui)

- A `ChangelogState` enum on the app model: `Idle`, `Loading`,
  `Loaded(Vec<ReleaseNote>)`, `Error(String)`.
- A `show_changelog: bool` route flag on the model: when true, the changelog
  screen replaces the normal view.
- New `Msg` variants:
  - `OpenChangelog` — set `Loading`, set `show_changelog = true`, close the
    Settings modal (the screen takes over the window), dispatch the fetch.
  - `ChangelogLoaded(Result<Vec<ReleaseNote>, String>)` — store `Loaded`/`Error`.
  - `CloseChangelog` — set `show_changelog = false` and reopen the Settings
    modal (the button lives in Settings; returning there preserves context,
    mirroring `ThemePicker`'s `return_to_settings`).

The fetch runs off-thread via `Task::perform(async { releases(10) ... }, …)`,
the same pattern as the update check (`detect_tools_task` / the check task).

## UI surfaces

- **Settings → Updates section** — a new **"View changelog"** control (a row
  or button styled like the existing Updates actions) emitting
  `Msg::OpenChangelog`. Always available (not gated on update availability) —
  reading history is useful regardless.
- **Changelog screen** — a full-window view, *not* a centered modal overlay.
  In `view()`, before the normal body/modal composition, when `show_changelog`
  is set, return the changelog screen directly so it fills the window. Layout:
  - Header row: title "Changelog" + a back/close control (reuse the existing
    `close` SVG icon, or a "Back" text button styled like other modal actions)
    → `Msg::CloseChangelog`.
  - Body by state: `Loading` → spinner + "Loading…"; `Error(e)` → muted error
    text; `Loaded(notes)` empty → "No releases yet"; `Loaded(notes)` → a
    `scrollable` column of entries. Each entry: a header line combining `tag`,
    `name`, and `date` (omitting `date` when empty), then `clean_markdown(body)`
    rendered as muted text, with a thin separator between entries.
- **Escape** closes the changelog screen (→ `CloseChangelog`), consistent with
  modal dismissal.

Glyph note: reuse existing SVG icons (`icons.rs`) and `spinner`; introduce no
new Unicode symbols from the U+25xx/U+28xx ranges the bundled fonts lack. The
`• ` bullet from `clean_markdown` uses U+2022, which is **confirmed present**
in both bundled fonts (IBMPlexSans + BlexMono); the `…` ellipsis (U+2026) used
elsewhere is also present. No fallback needed.

## Testing

- Unit tests in `upgrade.rs`: `clean_markdown` (heading strip, bullet
  normalization, trailing-whitespace trim, blank-run collapse, inline markup
  left untouched) and the list parse (multiple entries newest-first, `name`
  fallback to tag, `date` prefix extraction, element missing `tag_name`
  skipped, empty array → empty vec).
- The GitHub list API call and the screen rendering are validated by manual
  runs, not automated tests (same rationale as the self-update network/UI
  paths).

## Out of scope

- Full Markdown rendering (bold/italic/links/tables/code blocks).
- Pagination beyond the first page (~10 most recent is enough for this
  project's history).
- Caching release notes to disk — re-fetched each time the screen opens.
- Filtering prereleases out of the history view.
