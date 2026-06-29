# Changelog Viewer Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an in-app changelog screen, opened from a "View changelog" button under Settings → Updates, listing the ~10 most recent GitHub releases with lightly-cleaned notes.

**Architecture:** New data logic (`releases()`, `clean_markdown()`, `ReleaseNote`) lives in the existing iced-free `src/upgrade.rs` (unit-testable). The gui fetches off-thread via `Task::perform`, holds results in a `ChangelogState`, and renders a full-window screen routed at the top of `view()` (bypassing the normal body/modal overlay).

**Tech Stack:** Rust, iced 0.13 (tokio executor), `ureq` (rustls), `serde_json`. No new dependencies.

## Global Constraints

- **No new dependencies** — reuse `ureq` (rustls) + `serde_json` already present. No date crate, no Markdown crate.
- **`src/upgrade.rs` stays iced-free** — pure Rust + `ureq` + `semver` + `anyhow` + `serde_json` only.
- **GitHub list endpoint:** `GET https://api.github.com/repos/gitfudge0/grove/releases?per_page=10`, headers `User-Agent: grove` and `Accept: application/vnd.github+json`, via a timeout-bounded `ureq` agent (connect 10s, read 30s) — same as `latest()`.
- **Date without a crate:** `published_at` is ISO 8601; `date` = its first 10 chars (`YYYY-MM-DD`), `""` if absent or shorter.
- **Markdown:** light cleanup only (strip leading `#` headings, normalize `-`/`*`/`+` bullets to `• `, trim trailing whitespace, collapse blank-line runs). Leave inline markup (`**bold**`, `` `code` ``) untouched. No full rendering.
- **Glyph rule:** reuse existing SVG icons (`icons.rs`) + `spinner`. The `•` bullet (U+2022) and `…` ellipsis (U+2026) are CONFIRMED present in both bundled fonts — safe to use. Introduce no U+25xx/U+28xx symbols.
- **Network/parse failure is non-fatal** — surfaces as an inline `Error` state on the changelog screen (user-initiated action, so the error is shown).
- **List endpoint includes prereleases** — that is intentional for the history view; no filtering. (Only the update-offer path stays on `/latest`.)

---

## File Structure

- **Modify `src/upgrade.rs`** — add `ReleaseNote`, `parse_releases` (pure), `releases(limit)` (network), `clean_markdown` (pure), and unit tests.
- **Modify `src/gui/state.rs`** — add `ChangelogState` enum; `Grove` fields `changelog: ChangelogState` and `show_changelog: bool`; new `Msg` variants; initializers.
- **Modify `src/gui/update.rs`** — `OpenChangelog`/`ChangelogLoaded`/`CloseChangelog` handlers, the fetch task, and Escape handling for the changelog route.
- **Modify `src/gui/view.rs`** — the full-window `changelog_screen()` view + the route check at the top of `view()`, and the "View changelog" button in the Settings Updates section.

---

### Task 1: `upgrade` — `ReleaseNote`, `releases()`, `clean_markdown()` (unit-tested)

**Files:**
- Modify: `src/upgrade.rs` (add types + functions after the existing `latest()`/`apply()` code, before the `#[cfg(test)]` module; add the new tests inside the existing `tests` module)

**Interfaces:**
- Consumes: nothing new (reuses the timeout-agent pattern from `latest()`).
- Produces:
  - `pub struct ReleaseNote { pub tag: String, pub name: String, pub date: String, pub body: String }` (derives `Debug, Clone`)
  - `pub fn releases(limit: usize) -> anyhow::Result<Vec<ReleaseNote>>`
  - `pub fn clean_markdown(input: &str) -> String`
  - (module-private, tested via `use super::*`) `fn parse_releases(json: &str, limit: usize) -> anyhow::Result<Vec<ReleaseNote>>`

- [ ] **Step 1: Write the failing tests**

Add to the existing `#[cfg(test)] mod tests` in `src/upgrade.rs`:

```rust
#[test]
fn clean_markdown_strips_headings() {
    assert_eq!(clean_markdown("## Features"), "Features");
    assert_eq!(clean_markdown("# Title"), "Title");
    // No space after # → not a heading, left as-is.
    assert_eq!(clean_markdown("#NoSpace"), "#NoSpace");
}

#[test]
fn clean_markdown_normalizes_bullets() {
    assert_eq!(clean_markdown("- item"), "• item");
    assert_eq!(clean_markdown("* item"), "• item");
    assert_eq!(clean_markdown("+ item"), "• item");
    // Indentation preserved on nested bullets.
    assert_eq!(clean_markdown("  - nested"), "  • nested");
}

#[test]
fn clean_markdown_trims_trailing_ws_and_collapses_blanks() {
    assert_eq!(clean_markdown("text   "), "text");
    assert_eq!(clean_markdown("a\n\n\n\nb"), "a\n\nb");
    // Leading/trailing blank lines removed.
    assert_eq!(clean_markdown("\n\nhello\n\n"), "hello");
}

#[test]
fn clean_markdown_leaves_inline_markup() {
    assert_eq!(clean_markdown("**bold** and `code`"), "**bold** and `code`");
}

#[test]
fn parse_releases_extracts_and_orders() {
    let json = r#"[
        {"tag_name":"v0.25.0","name":"Self-update","published_at":"2026-06-29T12:00:00Z","body":"notes"},
        {"tag_name":"v0.24.0","name":"","published_at":"2026-05-01T08:00:00Z","body":""}
    ]"#;
    let v = parse_releases(json, 10).unwrap();
    assert_eq!(v.len(), 2);
    assert_eq!(v[0].tag, "v0.25.0");
    assert_eq!(v[0].name, "Self-update");
    assert_eq!(v[0].date, "2026-06-29");
    assert_eq!(v[0].body, "notes");
    // Empty name falls back to tag.
    assert_eq!(v[1].name, "v0.24.0");
}

#[test]
fn parse_releases_name_null_falls_back_to_tag() {
    let json = r#"[{"tag_name":"v1.0.0","name":null,"published_at":"2026-01-01T00:00:00Z","body":""}]"#;
    let v = parse_releases(json, 10).unwrap();
    assert_eq!(v[0].name, "v1.0.0");
}

#[test]
fn parse_releases_missing_published_at_yields_empty_date() {
    let json = r#"[{"tag_name":"v1.0.0","body":""}]"#;
    let v = parse_releases(json, 10).unwrap();
    assert_eq!(v[0].date, "");
}

#[test]
fn parse_releases_skips_elements_without_tag() {
    let json = r#"[{"name":"no tag","body":""},{"tag_name":"v1.0.0","body":""}]"#;
    let v = parse_releases(json, 10).unwrap();
    assert_eq!(v.len(), 1);
    assert_eq!(v[0].tag, "v1.0.0");
}

#[test]
fn parse_releases_respects_limit() {
    let json = r#"[
        {"tag_name":"v3","body":""},
        {"tag_name":"v2","body":""},
        {"tag_name":"v1","body":""}
    ]"#;
    let v = parse_releases(json, 2).unwrap();
    assert_eq!(v.len(), 2);
    assert_eq!(v[0].tag, "v3");
    assert_eq!(v[1].tag, "v2");
}

#[test]
fn parse_releases_empty_array() {
    let v = parse_releases("[]", 10).unwrap();
    assert!(v.is_empty());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib upgrade::tests::clean_markdown_strips_headings`
Expected: FAIL — `cannot find function 'clean_markdown'`.

- [ ] **Step 3: Implement the types and functions**

Add to `src/upgrade.rs` (after `apply()`'s helpers, before `#[cfg(test)]`):

```rust
/// A single release's notes, for the in-app changelog screen.
#[derive(Debug, Clone)]
pub struct ReleaseNote {
    pub tag: String,
    pub name: String,
    pub date: String,
    pub body: String,
}

/// Parse a GitHub `/releases` JSON array into up to `limit` `ReleaseNote`s,
/// preserving GitHub's newest-first order. Elements missing `tag_name` are
/// skipped rather than failing the whole list.
fn parse_releases(json: &str, limit: usize) -> Result<Vec<ReleaseNote>> {
    let v: serde_json::Value = serde_json::from_str(json).context("parse releases json")?;
    let arr = v.as_array().ok_or_else(|| anyhow!("releases json is not an array"))?;
    let mut out = Vec::new();
    for el in arr {
        let Some(tag) = el.get("tag_name").and_then(|t| t.as_str()) else {
            continue; // skip malformed element
        };
        let tag = tag.to_string();
        let name = el
            .get("name")
            .and_then(|n| n.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or(&tag)
            .to_string();
        let date = el
            .get("published_at")
            .and_then(|p| p.as_str())
            .filter(|s| s.len() >= 10)
            .map(|s| s[..10].to_string())
            .unwrap_or_default();
        let body = el
            .get("body")
            .and_then(|b| b.as_str())
            .unwrap_or("")
            .to_string();
        out.push(ReleaseNote { tag, name, date, body });
        if out.len() >= limit {
            break;
        }
    }
    Ok(out)
}

/// Fetch up to `limit` recent releases from GitHub for the changelog screen.
/// Blocks the calling thread — call from a background thread, never the UI thread.
pub fn releases(limit: usize) -> Result<Vec<ReleaseNote>> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(10))
        .timeout_read(std::time::Duration::from_secs(30))
        .build();
    let url = format!(
        "https://api.github.com/repos/gitfudge0/grove/releases?per_page={limit}"
    );
    let body = agent
        .get(&url)
        .set("User-Agent", "grove")
        .set("Accept", "application/vnd.github+json")
        .call()
        .context("github releases list request failed")?
        .into_string()
        .context("read github releases list body")?;
    parse_releases(&body, limit)
}

/// Light, dependency-free Markdown cleanup for display: strip ATX headings,
/// normalize unordered-list markers to `• `, trim trailing whitespace, and
/// collapse runs of blank lines. Inline markup is left untouched.
pub fn clean_markdown(input: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut prev_blank = false;
    for raw in input.lines() {
        let line = raw.trim_end();
        let trimmed = line.trim_start();
        let indent = &line[..line.len() - trimmed.len()];

        let cleaned = if trimmed.starts_with('#') {
            // ATX heading only when the `#` run is followed by a space.
            let after_hashes = trimmed.trim_start_matches('#');
            if after_hashes.starts_with(' ') {
                after_hashes.trim_start().to_string()
            } else {
                line.to_string()
            }
        } else if let Some(rest) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
            .or_else(|| trimmed.strip_prefix("+ "))
        {
            format!("{indent}• {rest}")
        } else {
            line.to_string()
        };

        let is_blank = cleaned.trim().is_empty();
        if is_blank && prev_blank {
            continue;
        }
        prev_blank = is_blank;
        lines.push(cleaned);
    }
    // Trim leading/trailing blank lines.
    while lines.first().map(|l| l.trim().is_empty()).unwrap_or(false) {
        lines.remove(0);
    }
    while lines.last().map(|l| l.trim().is_empty()).unwrap_or(false) {
        lines.pop();
    }
    lines.join("\n")
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib upgrade::`
Expected: PASS — all existing upgrade tests plus the 10 new ones. Then `cargo build` clean.

- [ ] **Step 5: Commit**

```bash
git add src/upgrade.rs
git commit -m "feat(changelog): add releases() and clean_markdown to upgrade module"
```

---

### Task 2: gui state, messages, fetch task, handlers

**Files:**
- Modify: `src/gui/state.rs` — add `ChangelogState`; `Grove` fields `changelog`, `show_changelog`; `Msg` variants; initializers (alongside the existing `upgrade`/`settings_tools` fields and their initializers).
- Modify: `src/gui/update.rs` — fetch task + `OpenChangelog`/`ChangelogLoaded`/`CloseChangelog` handlers + Escape route handling.

**Interfaces:**
- Consumes: `crate::upgrade::{ReleaseNote, releases}`; the `Task::perform` off-thread pattern used by the update check; `crate::app::Modal::Settings` and `Modal::None`.
- Produces:
  - `pub enum ChangelogState { Idle, Loading, Loaded(Vec<crate::upgrade::ReleaseNote>), Error(String) }` (derives `Debug, Clone`)
  - `Grove.changelog: ChangelogState`, `Grove.show_changelog: bool`
  - `Msg::OpenChangelog`, `Msg::ChangelogLoaded(Result<Vec<crate::upgrade::ReleaseNote>, String>)`, `Msg::CloseChangelog`

- [ ] **Step 1: Add `ChangelogState` and `Grove` fields**

In `src/gui/state.rs`, near `UpgradeState`:

```rust
/// Drives the full-window changelog screen.
#[derive(Debug, Clone)]
pub enum ChangelogState {
    Idle,
    Loading,
    Loaded(Vec<crate::upgrade::ReleaseNote>),
    Error(String),
}
```

Add to the `Grove` struct (alongside `upgrade`):

```rust
    pub changelog: ChangelogState,
    /// When true, the changelog screen replaces the normal view.
    pub show_changelog: bool,
```

Initialize where `Grove` is constructed (beside the `upgrade: UpgradeState::Idle` initializer):

```rust
    changelog: ChangelogState::Idle,
    show_changelog: false,
```

- [ ] **Step 2: Add `Msg` variants**

In the `Msg` enum (`src/gui/state.rs`), near the update messages:

```rust
    OpenChangelog,
    ChangelogLoaded(Result<Vec<crate::upgrade::ReleaseNote>, String>),
    CloseChangelog,
```

- [ ] **Step 3: Add the fetch task and handlers in `update.rs`**

Add a fetch-task method near `check_updates_task` (`src/gui/update.rs`):

```rust
fn fetch_changelog_task(&self) -> Task<Msg> {
    // Off-thread, mirroring the update check. 10 most recent releases.
    Task::perform(
        async { crate::upgrade::releases(10).map_err(|e| e.to_string()) },
        Msg::ChangelogLoaded,
    )
}
```

Add handlers in the `update()` match (near the update-check handlers):

```rust
Msg::OpenChangelog => {
    self.changelog = ChangelogState::Loading;
    self.show_changelog = true;
    // The full-window screen takes over; close the Settings modal behind it.
    self.app.modal = crate::app::Modal::None;
    return self.fetch_changelog_task();
}
Msg::ChangelogLoaded(result) => {
    self.changelog = match result {
        Ok(notes) => ChangelogState::Loaded(notes),
        Err(e) => ChangelogState::Error(e),
    };
}
Msg::CloseChangelog => {
    self.show_changelog = false;
    // Return to Settings, where the button lives (mirrors ThemePicker return).
    self.app.modal = crate::app::Modal::Settings;
}
```

- [ ] **Step 4: Add Escape handling for the changelog route**

The changelog is a route, not a modal, so handle Escape before the modal key logic. At the start of the key-handling path in `update.rs` (the `Msg::KeyPress`/key arm — find where `Key::Named(Named::Escape)` is matched for modals, ~line 1384+ region), add an early branch:

```rust
// Changelog is a full-window route; Escape returns to Settings.
if self.show_changelog {
    if matches!(key, Key::Named(Named::Escape)) {
        self.show_changelog = false;
        self.app.modal = crate::app::Modal::Settings;
    }
    return Task::none();
}
```

Place this so it runs before the modal-Escape handling and does not interfere with normal key routing when `show_changelog` is false. Read the surrounding key handler and integrate cleanly (match the real variable names for the key and modifiers).

- [ ] **Step 5: Verify it compiles**

Run: `cargo build`
Expected: clean build. `cargo test` still passes (Task 1's tests; no new automated tests here — gui wiring). NOTE: `Msg::OpenChangelog` has no view call site until Task 3; ensure the match is exhaustive and the build is warning-free (these variants are referenced by the handlers above and by Task 3's view).

- [ ] **Step 6: Commit**

```bash
git add src/gui/state.rs src/gui/update.rs
git commit -m "feat(changelog): gui state, messages, fetch task, handlers"
```

---

### Task 3: Full-window changelog screen + "View changelog" button

**Files:**
- Modify: `src/gui/view.rs` — add `changelog_screen()`; route to it at the top of `view()` (~line 80, before the `let body = …` / modal composition at lines 80-104); add the "View changelog" button to the Settings Updates section (`settings_modal`, the `updates_section` built ~line 2415-2511).

**Interfaces:**
- Consumes: `ChangelogState`, `Grove.show_changelog`, `crate::upgrade::{ReleaseNote, clean_markdown}`; existing view helpers — `scrollable` (already imported, line 26), `column`, `row`, `container`, `text`, `Space`, `icon`/`icon_btn`, `super::icons::spinner(size, color, self.blink_tick)`, color fns `c::FG()/FG_DIM()/FG_MUTE()/MAGENTA()/BORDER()`, `modal_action(label, ModalBtn::_, msg)`, layout const `ROW_H`, `UI_BOLD`. Confirm exact `ModalBtn` variant names by reading the file.
- Produces: `Msg::OpenChangelog` (button), `Msg::CloseChangelog` (back control), and the rendered screen.

- [ ] **Step 1: Route to the changelog screen at the top of `view()`**

In `src/gui/view.rs`, at the start of `pub fn view(&self) -> Element<'_, Msg>` (before the existing `let body = …` at line 81), add:

```rust
if self.show_changelog {
    return self.changelog_screen();
}
```

- [ ] **Step 2: Implement `changelog_screen()`**

Add to `src/gui/view.rs` (near the other view functions; follow the existing helper idioms):

```rust
fn changelog_screen(&self) -> Element<'_, Msg> {
    // Header: title + back control.
    let header = row![
        text("Changelog").size(15).color(c::MAGENTA()),
        Space::with_width(Length::Fill),
        icon_btn("close", Msg::CloseChangelog),
    ]
    .align_y(Center)
    .padding(Padding::from([0, 4]));

    let inner: Element<'_, Msg> = match &self.changelog {
        ChangelogState::Idle | ChangelogState::Loading => row![
            super::icons::spinner(16.0, c::FG_DIM(), self.blink_tick),
            Space::with_width(10),
            text("Loading…").size(12).color(c::FG_MUTE()),
        ]
        .align_y(Center)
        .into(),
        ChangelogState::Error(e) => {
            text(format!("Couldn’t load changelog: {e}")).size(12).color(c::FG_MUTE()).into()
        }
        ChangelogState::Loaded(notes) if notes.is_empty() => {
            text("No releases yet.").size(12).color(c::FG_MUTE()).into()
        }
        ChangelogState::Loaded(notes) => {
            let mut list = Column::new().spacing(18);
            for n in notes {
                // Header line: tag · name · date (omit empties).
                let mut head = row![
                    text(n.tag.clone()).size(13).font(UI_BOLD).color(c::FG()),
                ]
                .spacing(8)
                .align_y(Center);
                if !n.name.is_empty() && n.name != n.tag {
                    head = head.push(text(n.name.clone()).size(13).color(c::FG_DIM()));
                }
                if !n.date.is_empty() {
                    head = head.push(Space::with_width(Length::Fill));
                    head = head.push(text(n.date.clone()).size(11).color(c::FG_MUTE()));
                }
                let body_text = crate::upgrade::clean_markdown(&n.body);
                let entry = column![
                    head,
                    Space::with_height(4),
                    text(body_text).size(12).color(c::FG_DIM()),
                ]
                .spacing(0);
                list = list.push(entry);
            }
            scrollable(list).width(Length::Fill).height(Length::Fill).into()
        }
    };

    let screen = column![
        header,
        Space::with_height(12),
        container(inner).width(Length::Fill).height(Length::Fill),
    ]
    .spacing(0)
    .padding(20);

    container(screen)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}
```

Adapt to the real helper signatures and imports you find (e.g. `Column`, `Center`, `Padding`, `Length` are already used throughout `view.rs`; reuse the same import paths). If `icon_btn("close", …)` is not the right dismiss affordance, use `modal_action("Back", ModalBtn::Plain, Msg::CloseChangelog)` instead.

- [ ] **Step 3: Add the "View changelog" button to the Updates section**

In `settings_modal()` (the `updates_section`/`updates_col` block, ~line 2415-2511 from the self-update feature), add a control emitting `Msg::OpenChangelog`. Place it as its own row below the status/action rows, always present (not gated on update availability):

```rust
let changelog_row = container(
    modal_action("View changelog", ModalBtn::Plain, Msg::OpenChangelog),
)
.padding(Padding::from([4, 10]));
updates_col = updates_col.push(changelog_row);
```

Match the real `ModalBtn` variant and `modal_action` arity used elsewhere in `settings_modal`. Ensure `updates_col` is the column actually pushed into the modal body.

- [ ] **Step 4: Verify it compiles**

Run: `cargo build && cargo test`
Expected: clean build (no new warnings), all tests pass (Task 1's unit tests + existing 138). Manually confirm later: Settings → "View changelog" opens a full-window list; Escape / close returns to Settings; Loading shows a spinner; long lists scroll.

- [ ] **Step 5: Commit**

```bash
git add src/gui/view.rs
git commit -m "feat(changelog): full-window changelog screen and Settings button"
```

---

## Self-Review (author checklist — completed)

**Spec coverage:**
- `releases(limit)` + `ReleaseNote` + `parse_releases` → Task 1. ✓
- `clean_markdown` (heading/bullet/trim/collapse, inline left) → Task 1. ✓
- Date-prefix-without-crate → Task 1 (`parse_releases`). ✓
- Prereleases included (no filter) → Task 1 (no filtering applied). ✓
- `ChangelogState` + `show_changelog` + 3 Msg variants → Task 2. ✓
- Off-thread fetch mirroring the check → Task 2 (`fetch_changelog_task`). ✓
- Open closes Settings; Close reopens Settings → Task 2 handlers. ✓
- Full-window route at top of `view()` → Task 3. ✓
- Header + back control + state-based body + scrollable list → Task 3. ✓
- "View changelog" button under Updates, always available → Task 3. ✓
- Escape closes screen → Task 2 Step 4. ✓
- Glyph rule (reuse icons/spinner; `•`/`…` confirmed present) → Task 3 + Task 1. ✓
- Non-fatal error shown inline → Task 2 (`Error`) + Task 3 (Error render). ✓
- No new deps → all tasks reuse ureq/serde_json. ✓

**Type consistency:** `ReleaseNote`, `ChangelogState`, `releases(limit)`, `clean_markdown`, and the three `Msg` variants are referenced with identical names/types across Tasks 1-3.

**Known adaptation points (read the file, match exact signatures):** `ModalBtn` variant names, `modal_action` arity, `icon_btn` signature, the `view()` entry and the `updates_col` variable name, and the key-handler variable names for Escape. All anchored with file:line.
