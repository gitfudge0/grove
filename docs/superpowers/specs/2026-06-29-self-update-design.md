# Self-Update — Design

## Goal

Let Grove check GitHub releases for a newer version, surface it to the user, and
apply the upgrade in place. GitHub releases are the only update channel today
and there is no in-app signal that a new version exists. After this change Grove
checks on launch, on demand, and once a day; detects how the running binary was
installed; and performs the upgrade in the background, ending with a prompt to
restart.

The design keeps all update logic in one new module with no `iced` dependency,
so it can be unit-tested standalone. The GUI layer only orchestrates it.

## Module boundary

A new `src/upgrade.rs` (registered in `main.rs` alongside the other top-level
modules) owns three concerns and depends on nothing in `gui/`:

- **`detect() -> InstallMethod`** — classify the running install from
  `std::env::current_exe()`.
- **`latest() -> Result<Release>`** — query the GitHub releases API and return
  the latest stable release.
- **`apply(method, release, progress) -> Result<()>`** — perform the upgrade for
  the detected method, reporting stage transitions through a progress channel.

Naming note: `src/gui/update.rs` already exists and is the `iced` message loop
(`fn update`), unrelated to app-updating. The feature module is therefore named
**`upgrade`**, never `update`, to avoid confusion.

## New dependencies

- **`ureq`** with the `rustls` TLS feature — blocking HTTP, pure-Rust, no
  OpenSSL. Used for both the JSON API call and downloading the `.dmg` artifact.
  Blocking is fine because every call runs on a background thread (an `iced`
  `Task::perform` future or a spawned thread), never the UI thread.
- **`semver`** — parse and compare release tags (`v0.24.0`, `v0.18.0-beta.1`)
  correctly instead of hand-rolled string comparison.

## Install-method detection

`detect()` inspects the canonicalized path of `std::env::current_exe()`:

- Path under `~/.cargo/bin` → **`Source`** — installed via `cargo install`
  (what `install.sh` does). Upgraded by rebuilding from source.
- Path inside a macOS `.app` bundle (contains `/Contents/MacOS/`) → **`Dmg`** —
  installed from the release `.dmg`.
- Otherwise on Linux (e.g. under `/usr`) → **`Deb`** — installed from the `.deb`.
- Anything we cannot classify → **`Unknown`** — notify-only; the apply button is
  hidden and the notice links to the releases page.

`Deb` is upgraded through the **same source-rebuild path as `Source`** (see
below), so detection only needs to be accurate enough to distinguish `Dmg` from
"rebuild from source".

## Version check

A single check function is shared by all three triggers:

1. `latest()` issues `GET
   https://api.github.com/repos/gitfudge0/grove/releases/latest` with a
   `User-Agent` header (GitHub requires one) and `Accept:
   application/vnd.github+json`. The `/latest` endpoint **excludes
   prereleases**, which matches the project's convention that only full releases
   ship from `main`. Stable installs are therefore never offered a beta.
2. Parse the response `tag_name` (strip a leading `v`) into a `semver::Version`.
   Keep `html_url` (release page) and `body` (release notes) for display.
3. Compare against `env!("CARGO_PKG_VERSION")`. If the release is strictly
   greater **and** is not the user's skipped version (below), an update is
   available.

Network/parse failures are non-fatal: the check logs and reports a soft error
state. A manual check surfaces the error inline; launch/periodic checks fail
silently.

## Triggers

All three call the same check function:

- **On launch** — fire roughly 3s after startup (a delayed `Task`), silent
  unless an update is available.
- **Manual** — a "Check for updates" control in the Settings modal, beside the
  existing Tools section, showing live status (checking / up to date / available
  / error).
- **Periodic** — re-check at most once per 24h while running, gated by a
  `last_update_check` timestamp persisted in `Store`. The launch check also
  updates this timestamp so the two cooperate.

The check runs off-thread via `Task::perform` and posts results back as a new
`Msg` variant, mirroring how the Tools section already runs its `agent
--version` scan off-thread and posts `Msg::ToolVersionsDetected`.

## Persistence (`Store`)

Two new fields on `storage::Store`, both `#[serde(default)]` so existing config
files load unchanged:

- `last_update_check: Option<i64>` — Unix timestamp (seconds) of the last
  completed check; gates the periodic trigger.
- `skipped_version: Option<String>` — the release tag the user chose to skip.
  While the latest release equals this value, no notice is shown. A newer
  release than the skipped one clears the suppression and surfaces again.

Saved through the existing `storage::save` path, consistent with how every other
setting persists immediately on change.

## Apply flow

`apply()` runs on a background thread and reports progress through stages:
**Downloading / Building → Installing → Done**. Per method:

- **Source / Deb** — shallow-clone the latest release tag into a temp dir
  (`git clone --depth 1 --branch <tag>`), then `cargo install --path . --force`.
  This replaces `~/.cargo/bin/grove` atomically (cargo writes to a temp path and
  renames). The toolchain is guaranteed present for `Source`; for `Deb` it is
  assumed present, and if `cargo`/`git` is missing we abort with a clear error
  and fall back to notify + show-command (the `install.sh` one-liner). This path
  avoids root entirely — no `dpkg`, no `pkexec`.
- **Dmg (macOS)** — download the release `.dmg` asset to a temp file, `hdiutil
  attach` it, copy the `.app` over the currently running bundle
  (`cp -R`/`ditto`), then `hdiutil detach`. The running process keeps its open
  file handles; the new binary takes effect on the next launch.
- **Unknown** — no apply; notify-only.

Errors at any stage stop the flow and surface in the progress UI with the
failing step and message; the temp dir / mounted volume is cleaned up.

## UI surfaces

- **Settings modal** — a new **Updates** section (sibling to Tools) showing the
  current version, the check status, and, when an update is available, the new
  version with **Update now**, **Check for updates**, and **Skip this version**
  actions, plus a link to the release notes. "Skip this version" sets
  `skipped_version` and dismisses the notice.
- **Main view badge** — after the launch check finds an update, a subtle
  "update available" affordance appears (e.g. a marker on the cog/Settings
  entry point) so the user notices without an intrusive popup. It respects
  `skipped_version`.
- **Progress modal** — opened when the user starts an update. A new
  `Modal::Updating` variant rendered through the existing one-deep modal overlay
  (same pattern as Settings/ThemePicker). It shows the current stage with a
  spinner, and on completion shows **"Update installed — restart grove to
  apply"** with a **Restart** button. There is **no auto-restart** — active PTY
  sessions make that unsafe; the user controls timing.

Glyph note: any new status symbols must be checked against the bundled fonts'
cmap before use (the bundled fonts lack U+25xx/U+28xx glyphs); prefer existing
SVG icons (`icons.rs`) over new Unicode symbols.

## State & messages (gui)

- An `UpgradeState` enum on the app state captures: `Idle`, `Checking`,
  `UpToDate`, `Available(Release)`, `Error(String)`, and during apply
  `Updating(stage)` / `Updated` / `UpdateFailed(String)`.
- New `Msg` variants drive it: trigger a check, receive a check result, start an
  update, receive stage updates, receive completion, skip a version, restart.
- The check and apply tasks are dispatched with `Task::perform` (check) and a
  background thread streaming progress messages (apply), consistent with
  existing async patterns in `gui/update.rs`.

## Testing

- Unit tests in `upgrade.rs`: version comparison (newer / equal / older /
  prerelease ignored), `skipped_version` suppression logic, tag parsing (with
  and without leading `v`), and `InstallMethod` classification given sample exe
  paths.
- The GitHub API call and the apply routines (which shell out / touch the
  filesystem) are validated by manual end-to-end runs on macOS (Dmg + Source)
  and Linux (Deb-via-source), not automated tests.

## Out of scope

- Auto-restart after update.
- Update channels other than GitHub releases (e.g. Homebrew, cargo registry).
- Cryptographic signature verification of artifacts — no signing infrastructure
  exists; HTTPS to GitHub is the trust boundary. Noted as a known limitation.
- Delta/incremental updates.
