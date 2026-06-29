# Self-Update Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let Grove detect a newer GitHub release, surface it in the UI, and apply the upgrade in place (rebuild-from-source or `.dmg` replacement), ending with a restart prompt.

**Architecture:** All update logic lives in a new iced-free `src/upgrade.rs` module (detect / latest / apply) so it is unit-testable standalone. The gui layer orchestrates it via `Task::perform` for the check and a background thread (writing progress into a shared `Arc<Mutex<…>>` polled on `Tick`, mirroring the existing `bg_status` pattern) for the apply.

**Tech Stack:** Rust, iced 0.13 (tokio executor), `ureq` (rustls TLS) for HTTP, `semver` for version parsing, `serde`/`serde_json` for persistence.

## Global Constraints

- **Module name is `upgrade`, never `update`** — `src/gui/update.rs` is the iced message loop (`fn update`) and is unrelated. Do not add app-update logic there beyond message dispatch.
- **`src/upgrade.rs` must not depend on `iced`** — pure Rust + `ureq` + `semver` + `anyhow` only, so it unit-tests standalone.
- **New deps only:** `ureq` with the `rustls` feature, and `semver`. Do not add `reqwest`, `openssl`, `tempfile`, `rand`, or any other crate.
- **GitHub API:** `GET https://api.github.com/repos/gitfudge0/grove/releases/latest`, with headers `User-Agent: grove` and `Accept: application/vnd.github+json`. The `/latest` endpoint excludes prereleases — stable installs are never offered a beta.
- **Clone URL for source rebuild:** `https://github.com/gitfudge0/grove.git`.
- **Current version source:** `env!("CARGO_PKG_VERSION")` (compile-time; currently `0.24.0`).
- **Persistence:** every new `Store` field is `#[serde(default)]` so existing `~/.config/grove/projects.json` files load unchanged; persist through `storage::save`.
- **Network/parse failures are non-fatal.** Manual check surfaces the error inline; launch/periodic checks fail silently (log only).
- **No auto-restart.** Active PTY sessions make it unsafe; completion shows a manual **Restart** button.
- **Glyph rule:** the bundled fonts lack U+25xx/U+28xx glyphs. Do not introduce new Unicode status symbols — reuse existing SVG icons from `src/gui/icons.rs` (`icon(name, size, color)`, `spinner(size, color, tick)`, `dot(color)`).
- **`Deb` upgrades via the same source-rebuild path as `Source`.** Detection only needs to be accurate enough to distinguish `Dmg` from rebuild-from-source.

---

## File Structure

- **Create `src/upgrade.rs`** — the iced-free update module: `InstallMethod`, `Release`, `Stage`, pure helpers (`classify_path`, `parse_release`, `update_available`), `detect()`, `latest()`, `apply()`, and unit tests.
- **Modify `src/main.rs`** — register `mod upgrade;`.
- **Modify `Cargo.toml`** — add `ureq` and `semver`.
- **Modify `src/storage.rs`** — add `last_update_check` and `skipped_version` fields to `Store`; serde test.
- **Modify `src/gui/state.rs`** — add `UpgradeState`, new `Msg` variants, and fields on `Grove`.
- **Modify `src/gui/update.rs`** — check `Task`, apply dispatch, result/skip/restart/stage handlers, launch + periodic triggers.
- **Modify `src/gui/view.rs`** — Settings "Updates" section, cog badge, `Modal::Updating` progress modal.
- **Modify `src/app.rs`** — add `Modal::Updating` variant and the apply-progress shared handle.

---

### Task 1: `upgrade` module — dependencies, types, detection, parsing, version logic (unit-tested core)

**Files:**
- Modify: `Cargo.toml` (`[dependencies]`, after line 33 `iced = …`)
- Modify: `src/main.rs:1-11` (module declarations block — insert `mod upgrade;` alphabetically after `mod tmux;`/before `mod theme;` — keep the block sorted: current order is `agent, app, clipboard, env_path, git, gui, session, session_meta, storage, theme, tmux`; insert `upgrade` after `tmux`)
- Create: `src/upgrade.rs`
- Test: inline `#[cfg(test)] mod tests` in `src/upgrade.rs`

**Interfaces:**
- Produces:
  - `pub enum InstallMethod { Source, Dmg, Deb, Unknown }` (derives `Debug, Clone, Copy, PartialEq, Eq`)
  - `pub struct Release { pub version: semver::Version, pub tag: String, pub html_url: String, pub body: String, pub dmg_url: Option<String> }` (derives `Debug, Clone`)
  - `pub enum Stage { Downloading, Building, Installing, Done }` (derives `Debug, Clone, Copy, PartialEq, Eq`)
  - `pub fn detect() -> InstallMethod`
  - `pub fn latest() -> anyhow::Result<Release>`
  - `pub fn update_available(current: &str, release: &Release, skipped: Option<&str>) -> bool`
  - (pure internal, but `pub(crate)` for tests) `fn classify_path(exe: &std::path::Path, target_os: &str) -> InstallMethod`, `fn parse_release(json: &str) -> anyhow::Result<Release>`

- [ ] **Step 1: Add dependencies to `Cargo.toml`**

In the `[dependencies]` table (after the `iced = { … }` line at line 33), add:

```toml
ureq = { version = "2", features = ["tls"], default-features = false }
semver = "1"
```

Note: `ureq` 2.x uses rustls via the `tls` feature with `default-features = false` to avoid pulling native-tls/OpenSSL. Verify the resolved feature set after the build step; if `tls` is not the rustls feature name for the resolved version, use `features = ["rustls"]`. The binding requirement is rustls (pure-Rust), not OpenSSL.

- [ ] **Step 2: Register the module**

In `src/main.rs`, insert into the `mod` block (lines 1-11) so it reads `… mod tmux;` then `mod upgrade;` (keep `mod theme;` before `mod tmux;` as today; add `mod upgrade;` as the last module line):

```rust
mod tmux;
mod upgrade;
```

- [ ] **Step 3: Write the failing tests** (create `src/upgrade.rs` with types + stubs + tests)

Create `src/upgrade.rs`:

```rust
//! In-place self-update for Grove. Pure logic with no `iced` dependency so it
//! unit-tests standalone; the gui layer orchestrates it. Named `upgrade` (not
//! `update`) to avoid confusion with `gui::update`, the iced message loop.

use anyhow::{anyhow, Context, Result};
use std::path::Path;

/// How the running binary was installed. Determines the apply strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallMethod {
    /// `cargo install` into `~/.cargo/bin` (what `install.sh` does). Rebuild from source.
    Source,
    /// macOS `.app` bundle from the release `.dmg`. Replace the bundle.
    Dmg,
    /// Linux `.deb`. Upgraded via the same source-rebuild path as `Source`.
    Deb,
    /// Unclassifiable — notify only, no apply.
    Unknown,
}

/// A GitHub release resolved from the `/releases/latest` endpoint.
#[derive(Debug, Clone)]
pub struct Release {
    pub version: semver::Version,
    pub tag: String,
    pub html_url: String,
    pub body: String,
    /// `browser_download_url` of the first `.dmg` asset, if any.
    pub dmg_url: Option<String>,
}

/// Apply progress stages, reported through the `apply` callback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Downloading,
    Building,
    Installing,
    Done,
}

/// Classify an executable path. Pure and platform-injected so it tests on any host.
fn classify_path(exe: &Path, target_os: &str) -> InstallMethod {
    let s = exe.to_string_lossy();
    if s.contains("/.cargo/bin/") {
        InstallMethod::Source
    } else if s.contains("/Contents/MacOS/") {
        InstallMethod::Dmg
    } else if target_os == "linux" {
        InstallMethod::Deb
    } else {
        InstallMethod::Unknown
    }
}

/// Classify the running install from `current_exe()`.
pub fn detect() -> InstallMethod {
    match std::env::current_exe() {
        Ok(p) => {
            let canonical = std::fs::canonicalize(&p).unwrap_or(p);
            classify_path(&canonical, std::env::consts::OS)
        }
        Err(_) => InstallMethod::Unknown,
    }
}

/// Parse a GitHub `/releases/latest` JSON body into a `Release`.
fn parse_release(json: &str) -> Result<Release> {
    let v: serde_json::Value = serde_json::from_str(json).context("parse release json")?;
    let tag = v
        .get("tag_name")
        .and_then(|t| t.as_str())
        .ok_or_else(|| anyhow!("release json missing tag_name"))?
        .to_string();
    let version = semver::Version::parse(tag.trim_start_matches('v'))
        .with_context(|| format!("parse semver from tag {tag}"))?;
    let html_url = v
        .get("html_url")
        .and_then(|u| u.as_str())
        .unwrap_or("")
        .to_string();
    let body = v
        .get("body")
        .and_then(|b| b.as_str())
        .unwrap_or("")
        .to_string();
    let dmg_url = v
        .get("assets")
        .and_then(|a| a.as_array())
        .and_then(|assets| {
            assets.iter().find_map(|asset| {
                let name = asset.get("name").and_then(|n| n.as_str())?;
                if name.ends_with(".dmg") {
                    asset
                        .get("browser_download_url")
                        .and_then(|u| u.as_str())
                        .map(String::from)
                } else {
                    None
                }
            })
        });
    Ok(Release {
        version,
        tag,
        html_url,
        body,
        dmg_url,
    })
}

/// True when `release` is strictly newer than `current` and is not the skipped tag.
/// A release newer than the skipped tag has a different tag, so it surfaces again.
pub fn update_available(current: &str, release: &Release, skipped: Option<&str>) -> bool {
    let Ok(cur) = semver::Version::parse(current.trim_start_matches('v')) else {
        return false;
    };
    if release.version <= cur {
        return false;
    }
    if skipped == Some(release.tag.as_str()) {
        return false;
    }
    true
}

/// Query the GitHub releases API and return the latest stable release.
pub fn latest() -> Result<Release> {
    let body = ureq::get("https://api.github.com/repos/gitfudge0/grove/releases/latest")
        .set("User-Agent", "grove")
        .set("Accept", "application/vnd.github+json")
        .call()
        .context("github releases request failed")?
        .into_string()
        .context("read github response body")?;
    parse_release(&body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn release(tag: &str) -> Release {
        Release {
            version: semver::Version::parse(tag.trim_start_matches('v')).unwrap(),
            tag: tag.to_string(),
            html_url: String::new(),
            body: String::new(),
            dmg_url: None,
        }
    }

    #[test]
    fn classify_cargo_bin_is_source() {
        let p = PathBuf::from("/Users/x/.cargo/bin/grove");
        assert_eq!(classify_path(&p, "macos"), InstallMethod::Source);
    }

    #[test]
    fn classify_app_bundle_is_dmg() {
        let p = PathBuf::from("/Applications/Grove.app/Contents/MacOS/grove");
        assert_eq!(classify_path(&p, "macos"), InstallMethod::Dmg);
    }

    #[test]
    fn classify_linux_usr_is_deb() {
        let p = PathBuf::from("/usr/bin/grove");
        assert_eq!(classify_path(&p, "linux"), InstallMethod::Deb);
    }

    #[test]
    fn classify_unknown_when_unrecognized_non_linux() {
        let p = PathBuf::from("/opt/weird/grove");
        assert_eq!(classify_path(&p, "macos"), InstallMethod::Unknown);
    }

    #[test]
    fn update_available_when_strictly_newer() {
        assert!(update_available("0.24.0", &release("v0.25.0"), None));
    }

    #[test]
    fn no_update_when_equal() {
        assert!(!update_available("0.24.0", &release("v0.24.0"), None));
    }

    #[test]
    fn no_update_when_older() {
        assert!(!update_available("0.24.0", &release("v0.23.0"), None));
    }

    #[test]
    fn current_with_leading_v_is_parsed() {
        assert!(update_available("v0.24.0", &release("v0.25.0"), None));
    }

    #[test]
    fn skipped_tag_suppresses_same_release() {
        assert!(!update_available("0.24.0", &release("v0.25.0"), Some("v0.25.0")));
    }

    #[test]
    fn newer_than_skipped_surfaces_again() {
        // User skipped v0.25.0; v0.26.0 must still surface.
        assert!(update_available("0.24.0", &release("v0.26.0"), Some("v0.25.0")));
    }

    #[test]
    fn prerelease_tag_parses_but_is_not_offered_to_stable() {
        // /latest excludes prereleases, but guard the comparison regardless.
        let r = release("v0.25.0-beta.1");
        assert!(!update_available("0.25.0", &r, None));
    }

    #[test]
    fn parse_release_extracts_fields_and_dmg() {
        let json = r#"{
            "tag_name": "v0.25.0",
            "html_url": "https://github.com/gitfudge0/grove/releases/tag/v0.25.0",
            "body": "notes here",
            "assets": [
                {"name": "grove_0.25.0_amd64.deb", "browser_download_url": "https://x/d.deb"},
                {"name": "Grove.dmg", "browser_download_url": "https://x/Grove.dmg"}
            ]
        }"#;
        let r = parse_release(json).unwrap();
        assert_eq!(r.tag, "v0.25.0");
        assert_eq!(r.version, semver::Version::parse("0.25.0").unwrap());
        assert_eq!(r.body, "notes here");
        assert_eq!(r.dmg_url.as_deref(), Some("https://x/Grove.dmg"));
    }

    #[test]
    fn parse_release_no_dmg_asset_yields_none() {
        let json = r#"{"tag_name":"v0.25.0","html_url":"","body":"","assets":[]}"#;
        let r = parse_release(json).unwrap();
        assert!(r.dmg_url.is_none());
    }

    #[test]
    fn parse_release_missing_tag_errors() {
        let json = r#"{"html_url":"","body":""}"#;
        assert!(parse_release(json).is_err());
    }
}
```

- [ ] **Step 4: Run tests to verify they fail, then pass**

Run: `cargo test --lib upgrade::` (after writing the module above the tests pass; before, the module did not exist).
Expected: `test result: ok.` with all `upgrade::tests::*` passing. Also run `cargo build` — must compile clean (no `apply` yet; that is Task 2).

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock src/main.rs src/upgrade.rs
git commit -m "feat(upgrade): add update module with detection, parsing, version logic"
```

---

### Task 2: `upgrade::apply` — perform the upgrade per install method

**Files:**
- Modify: `src/upgrade.rs` (add `apply` + private helpers below `latest()`, before `#[cfg(test)]`)

**Interfaces:**
- Consumes: `InstallMethod`, `Release`, `Stage` (Task 1).
- Produces: `pub fn apply(method: InstallMethod, release: &Release, progress: &(dyn Fn(Stage) + Send + Sync)) -> anyhow::Result<()>`

**Note on testing:** `apply` shells out and touches the filesystem; it is validated by manual end-to-end runs (macOS Dmg + Source, Linux Deb-via-source), per the spec — no automated test. The reviewer gates on command correctness, error handling, and cleanup.

- [ ] **Step 1: Implement `apply` and helpers**

Add to `src/upgrade.rs` (after `latest()`):

```rust
use std::process::Command;

/// Perform the upgrade for the detected install method, reporting stage
/// transitions through `progress`. Runs on a background thread (blocking).
pub fn apply(
    method: InstallMethod,
    release: &Release,
    progress: &(dyn Fn(Stage) + Send + Sync),
) -> Result<()> {
    match method {
        InstallMethod::Source | InstallMethod::Deb => apply_source(release, progress),
        InstallMethod::Dmg => apply_dmg(release, progress),
        InstallMethod::Unknown => Err(anyhow!("unknown install method — cannot self-update")),
    }
}

/// True if `bin` is runnable (used to fail fast with a clear message).
fn have(bin: &str) -> bool {
    Command::new(bin)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Shallow-clone the release tag and `cargo install --path . --force`. Avoids root.
fn apply_source(release: &Release, progress: &(dyn Fn(Stage) + Send + Sync)) -> Result<()> {
    if !have("git") || !have("cargo") {
        return Err(anyhow!(
            "git and cargo are required to update from source. Reinstall with:\n  \
             curl -fsSL https://raw.githubusercontent.com/gitfudge0/grove/main/install.sh | sh"
        ));
    }

    let tmp = std::env::temp_dir().join(format!("grove-upgrade-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).context("create temp dir")?;
    // Clean up the temp dir no matter how we exit.
    let result = (|| -> Result<()> {
        progress(Stage::Downloading);
        run(
            Command::new("git").args([
                "clone",
                "--depth",
                "1",
                "--branch",
                &release.tag,
                "https://github.com/gitfudge0/grove.git",
                &tmp.to_string_lossy(),
            ]),
            "git clone",
        )?;

        progress(Stage::Building);
        run(
            Command::new("cargo").args([
                "install",
                "--path",
                &tmp.to_string_lossy(),
                "--force",
            ]),
            "cargo install",
        )?;

        progress(Stage::Done);
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(&tmp);
    result
}

/// Download the `.dmg`, mount it, copy the `.app` over the running bundle, detach.
fn apply_dmg(release: &Release, progress: &(dyn Fn(Stage) + Send + Sync)) -> Result<()> {
    let url = release
        .dmg_url
        .as_deref()
        .ok_or_else(|| anyhow!("release has no .dmg asset"))?;

    // Resolve the running .app bundle: strip "/Contents/MacOS/<bin>".
    let exe = std::env::current_exe().context("current_exe")?;
    let exe = std::fs::canonicalize(&exe).unwrap_or(exe);
    let exe_str = exe.to_string_lossy();
    let app_path = exe_str
        .split("/Contents/MacOS/")
        .next()
        .filter(|p| p.ends_with(".app"))
        .ok_or_else(|| anyhow!("could not resolve running .app bundle from {exe_str}"))?
        .to_string();

    let dmg_path = std::env::temp_dir().join(format!("grove-upgrade-{}.dmg", std::process::id()));
    let mnt = std::env::temp_dir().join(format!("grove-upgrade-mnt-{}", std::process::id()));

    let result = (|| -> Result<()> {
        progress(Stage::Downloading);
        let mut reader = ureq::get(url)
            .set("User-Agent", "grove")
            .call()
            .context("download dmg")?
            .into_reader();
        let mut file = std::fs::File::create(&dmg_path).context("create dmg file")?;
        std::io::copy(&mut reader, &mut file).context("write dmg file")?;
        drop(file);

        progress(Stage::Installing);
        std::fs::create_dir_all(&mnt).ok();
        run(
            Command::new("hdiutil").args([
                "attach",
                "-nobrowse",
                "-mountpoint",
                &mnt.to_string_lossy(),
                &dmg_path.to_string_lossy(),
            ]),
            "hdiutil attach",
        )?;

        // Find the .app inside the mounted volume.
        let app_in_dmg = std::fs::read_dir(&mnt)
            .context("read mounted volume")?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .find(|p| p.extension().map(|x| x == "app").unwrap_or(false))
            .ok_or_else(|| anyhow!("no .app found in mounted dmg"))?;

        // Replace the running bundle. `ditto` preserves macOS metadata.
        run(
            Command::new("ditto").args([
                &app_in_dmg.to_string_lossy(),
                &app_path,
            ]),
            "ditto copy",
        )?;

        progress(Stage::Done);
        Ok(())
    })();

    // Always detach + clean up, even on error.
    let _ = run(
        Command::new("hdiutil").args(["detach", &mnt.to_string_lossy()]),
        "hdiutil detach",
    );
    let _ = std::fs::remove_file(&dmg_path);
    result
}

/// Run a command, mapping non-zero exit (and spawn failure) to a contextual error.
fn run(cmd: &mut Command, what: &str) -> Result<()> {
    let out = cmd
        .output()
        .with_context(|| format!("failed to spawn {what}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(anyhow!("{what} failed: {}", stderr.trim()));
    }
    Ok(())
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo build`
Expected: clean build. Run `cargo test --lib upgrade::` — the Task 1 tests still pass (no new tests; `apply` is manually validated).

- [ ] **Step 3: Commit**

```bash
git add src/upgrade.rs
git commit -m "feat(upgrade): implement apply for source/deb rebuild and dmg replace"
```

---

### Task 3: Persistence — `last_update_check` and `skipped_version` on `Store`

**Files:**
- Modify: `src/storage.rs:30-53` (the `Store` struct; add two fields at the end, before the closing `}` at line 52)
- Test: inline `#[cfg(test)] mod tests` in `src/storage.rs`

**Interfaces:**
- Produces: `Store.last_update_check: Option<i64>`, `Store.skipped_version: Option<String>`.

- [ ] **Step 1: Write the failing test**

Add to the existing `storage::tests` module (the project already has `store_serde_round_trip` and `store_deserializes_from_empty_object`):

```rust
#[test]
fn store_loads_without_update_fields_and_defaults_them() {
    // Existing config files predate these fields; they must default to None.
    let store: Store = serde_json::from_str("{}").unwrap();
    assert!(store.last_update_check.is_none());
    assert!(store.skipped_version.is_none());
}

#[test]
fn store_round_trips_update_fields() {
    let mut store = Store::default();
    store.last_update_check = Some(1_700_000_000);
    store.skipped_version = Some("v0.25.0".to_string());
    let json = serde_json::to_string(&store).unwrap();
    let back: Store = serde_json::from_str(&json).unwrap();
    assert_eq!(back.last_update_check, Some(1_700_000_000));
    assert_eq!(back.skipped_version.as_deref(), Some("v0.25.0"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib storage::tests::store_round_trips_update_fields`
Expected: FAIL — `no field 'last_update_check' on type 'Store'`.

- [ ] **Step 3: Add the fields**

In `src/storage.rs`, append to the `Store` struct (after the `onboarded: bool` field, before the closing brace):

```rust
    /// Unix timestamp (seconds) of the last completed update check. Gates the
    /// periodic (24h) trigger. `#[serde(default)]` so old config files load.
    #[serde(default)]
    pub last_update_check: Option<i64>,
    /// The release tag the user chose to skip. While the latest release equals
    /// this value no update notice is shown; a newer release clears it.
    #[serde(default)]
    pub skipped_version: Option<String>,
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib storage::`
Expected: PASS — all storage tests including the two new ones.

- [ ] **Step 5: Commit**

```bash
git add src/storage.rs
git commit -m "feat(upgrade): persist last_update_check and skipped_version on Store"
```

---

### Task 4: gui plumbing — `UpgradeState`, messages, check task, triggers, persistence wiring

**Files:**
- Modify: `src/gui/state.rs` — add `UpgradeState` enum; add `Msg` variants (in the `Msg` enum, lines 262-450); add fields on `Grove` (struct lines 38-155) and initialize them where `Grove` is constructed.
- Modify: `src/gui/update.rs` — add the check `Task`, the result/skip handlers, and the launch + periodic triggers. The manual-check `Msg::CheckForUpdates` handler and `Msg::UpdateCheckResult`/`Msg::SkipVersion` go in the `update()` match (alongside `Msg::RefreshTools` at lines 742-754). The launch trigger is dispatched from app init; the periodic trigger is gated inside the existing `Msg::Tick` handler.

**Interfaces:**
- Consumes: `crate::upgrade::{Release, InstallMethod, update_available, latest, detect}`; `Store.last_update_check`, `Store.skipped_version`; existing `Task::perform` pattern from `detect_tools_task` (`src/gui/update.rs:855-888`).
- Produces:
  - `pub enum UpgradeState { Idle, Checking, UpToDate, Available(crate::upgrade::Release), Error(String), Updating(crate::upgrade::Stage), Updated, UpdateFailed(String) }` (derives `Debug, Clone`)
  - `Grove.upgrade: UpgradeState`, `Grove.upgrade_method: crate::upgrade::InstallMethod`, `Grove.upgrade_progress: std::sync::Arc<std::sync::Mutex<Option<crate::upgrade::Stage>>>`
  - `Msg::CheckForUpdates`, `Msg::UpdateCheckResult(Result<crate::upgrade::Release, String>)`, `Msg::SkipVersion`, `Msg::StartUpdate`, `Msg::UpdateStageChanged(crate::upgrade::Stage)`, `Msg::UpdateFinished(Result<(), String>)`, `Msg::RestartApp`

- [ ] **Step 1: Add `UpgradeState` and `Grove` fields**

In `src/gui/state.rs`, near the other gui types, add:

```rust
/// Drives the Updates UI. `Available` carries the resolved release; the apply
/// states drive the progress modal.
#[derive(Debug, Clone)]
pub enum UpgradeState {
    Idle,
    Checking,
    UpToDate,
    Available(crate::upgrade::Release),
    Error(String),
    Updating(crate::upgrade::Stage),
    Updated,
    UpdateFailed(String),
}
```

Add to the `Grove` struct (alongside `settings_tools`):

```rust
    pub upgrade: UpgradeState,
    pub upgrade_method: crate::upgrade::InstallMethod,
    /// Written by the apply thread, polled on `Tick` and mapped to `Msg::UpdateStageChanged`.
    pub upgrade_progress: std::sync::Arc<std::sync::Mutex<Option<crate::upgrade::Stage>>>,
```

Initialize them where `Grove` is constructed (the `settings_tools: Vec::new()` initializer is the anchor — add beside it):

```rust
    upgrade: UpgradeState::Idle,
    upgrade_method: crate::upgrade::detect(),
    upgrade_progress: std::sync::Arc::new(std::sync::Mutex::new(None)),
```

- [ ] **Step 2: Add `Msg` variants**

In the `Msg` enum (`src/gui/state.rs:262-450`), add near the Tools messages (`RefreshTools`, `ToolVersionsDetected`):

```rust
    CheckForUpdates,
    UpdateCheckResult(Result<crate::upgrade::Release, String>),
    SkipVersion,
    StartUpdate,
    UpdateStageChanged(crate::upgrade::Stage),
    UpdateFinished(Result<(), String>),
    RestartApp,
```

- [ ] **Step 3: Add the check task and handlers in `update.rs`**

Add a check-task method next to `detect_tools_task` (`src/gui/update.rs:855`):

```rust
fn check_updates_task(&mut self) -> Task<Msg> {
    self.upgrade = UpgradeState::Checking;
    // Mirrors detect_tools_task: short blocking work on the iced/tokio executor.
    Task::perform(
        async { crate::upgrade::latest().map_err(|e| e.to_string()) },
        Msg::UpdateCheckResult,
    )
}
```

Add handlers in the `update()` match (near `Msg::RefreshTools`, lines 742-754):

```rust
Msg::CheckForUpdates => return self.check_updates_task(),
Msg::UpdateCheckResult(result) => {
    // Record the check time regardless of outcome so the periodic trigger backs off.
    self.app.store.last_update_check = Some(now_unix());
    let _ = crate::storage::save(&self.app.store);
    match result {
        Ok(release) => {
            let current = env!("CARGO_PKG_VERSION");
            let skipped = self.app.store.skipped_version.as_deref();
            if crate::upgrade::update_available(current, &release, skipped) {
                self.upgrade = UpgradeState::Available(release);
            } else {
                self.upgrade = UpgradeState::UpToDate;
            }
        }
        Err(e) => {
            log::warn!("update check failed: {e}");
            self.upgrade = UpgradeState::Error(e);
        }
    }
}
Msg::SkipVersion => {
    if let UpgradeState::Available(release) = &self.upgrade {
        self.app.store.skipped_version = Some(release.tag.clone());
        let _ = crate::storage::save(&self.app.store);
    }
    self.upgrade = UpgradeState::UpToDate;
}
```

If the project has no `log` crate, replace `log::warn!(...)` with `eprintln!("update check failed: {e}")` — match whatever logging the codebase already uses (check `src/gui/update.rs` for existing `log::`/`eprintln!`/`tracing::` usage and use the same).

Add a `now_unix` helper near the other free functions in `update.rs`:

```rust
fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
```

- [ ] **Step 4: Wire the launch trigger (≈3s after startup)**

iced apps return an initial `Task` from their `new`/`boot` function. Find where `Grove` is constructed and an initial `Task` is returned (search `src/gui/update.rs` and `src/gui/state.rs` / `src/main.rs` for `iced::application` / the `new` returning `(Grove, Task<Msg>)` or `Task::none()` at boot). Add a delayed check to that initial task batch:

```rust
let launch_check = Task::perform(
    async {
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    },
    |_| Msg::CheckForUpdates,
);
// batch with the existing initial task:
Task::batch([existing_initial_task, launch_check])
```

If `new` currently returns `Task::none()`, return `launch_check` directly. `tokio` is already a dependency via iced's `tokio` feature.

- [ ] **Step 5: Wire the periodic trigger (≤ once / 24h) in the `Tick` handler**

In the existing `Msg::Tick` arm of `update()`, add a gate that fires a check at most once per 24h:

```rust
// Periodic update check: at most once per 24h while running.
{
    let due = match self.app.store.last_update_check {
        Some(ts) => now_unix() - ts >= 24 * 60 * 60,
        None => false, // launch check seeds the timestamp; don't double-fire at boot
    };
    if due && matches!(self.upgrade, UpgradeState::Idle | UpgradeState::UpToDate) {
        return self.check_updates_task();
    }
}
```

Place this so it does not shadow whatever the `Tick` arm already returns (append the periodic check as an early `return` only when `due`; otherwise fall through to the existing Tick logic). Read the current `Tick` arm and integrate without dropping existing behavior.

- [ ] **Step 6: Verify it compiles**

Run: `cargo build`
Expected: clean build. (No automated test — this is gui wiring; correctness is by the reviewer + manual run. The pure logic it calls is already tested in Task 1.)

- [ ] **Step 7: Commit**

```bash
git add src/gui/state.rs src/gui/update.rs
git commit -m "feat(upgrade): gui state, messages, check task, launch/periodic triggers"
```

---

### Task 5: Settings "Updates" section + cog badge

**Files:**
- Modify: `src/gui/view.rs` — add an Updates section inside `settings_modal()` (`src/gui/view.rs:2160-2396`) as a sibling to the Tools section, pushed onto the final `body` column (the `column![head, appearance, terminal, tools_section].spacing(16)` at the end of the function). Add a subtle badge on the cog/Settings entry point.

**Interfaces:**
- Consumes: `self.upgrade: UpgradeState`, `self.upgrade_method: InstallMethod`, `self.app.store.skipped_version`; the existing view helpers seen in `settings_modal`: `eyebrow(label)`, `caption(s)`, `modal_action(label, ModalBtn::_, msg)`, `icon(name, size, color)`, `spinner(size, color, tick)`, color fns `c::FG()/FG_DIM()/FG_MUTE()/GREEN()/MAGENTA()`, layout consts `ROW_H`. The current version is `env!("CARGO_PKG_VERSION")`.
- Produces: an `updates_section` rendered in the Settings modal; a badge affordance on the cog.

- [ ] **Step 1: Build the Updates section**

Inside `settings_modal()`, before the final `body` composition, build:

```rust
// ── updates ─────────────────────────────────────────────────────────
let current_ver = env!("CARGO_PKG_VERSION");
let status_line: Element<'_, Msg> = match &self.upgrade {
    UpgradeState::Idle => text("Not checked yet").size(12).color(c::FG_MUTE()).into(),
    UpgradeState::Checking => row![
        spinner(12.0, c::FG_MUTE(), self.tick),
        Space::with_width(8),
        text("Checking…").size(12).color(c::FG_MUTE()),
    ]
    .align_y(Center)
    .into(),
    UpgradeState::UpToDate => text("Up to date").size(12).color(c::FG_DIM()).into(),
    UpgradeState::Error(e) => text(format!("Check failed: {e}")).size(12).color(c::FG_MUTE()).into(),
    UpgradeState::Available(r) => {
        text(format!("Update available: {}", r.tag)).size(12).color(c::GREEN()).into()
    }
    // The apply states are shown in the progress modal, not here.
    _ => text("Updating…").size(12).color(c::FG_DIM()).into(),
};

let updates_header = container(
    row![
        text("UPDATES").font(UI_BOLD).size(11).color(c::FG_MUTE()),
        Space::with_width(Length::Fill),
        // Manual check control.
        if matches!(self.upgrade, UpgradeState::Checking) {
            container(spinner(13.0, c::FG_MUTE(), self.tick)).into()
        } else {
            icon_btn("restart", Msg::CheckForUpdates)
        },
    ]
    .align_y(Center),
)
.padding(Padding::from([0, 10]));

let current_row = container(
    row![
        text("Current version").size(12).color(c::FG()),
        Space::with_width(Length::Fill),
        text(format!("v{current_ver}")).size(12).color(c::FG_DIM()),
    ]
    .align_y(Center),
)
.height(ROW_H)
.padding(Padding::from([0, 10]));

let status_row = container(
    row![
        text("Status").size(12).color(c::FG()),
        Space::with_width(Length::Fill),
        status_line,
    ]
    .align_y(Center),
)
.height(ROW_H)
.padding(Padding::from([0, 10]));

// Action row, only when an update is available.
let mut updates_col = column![updates_header, current_row, status_row].spacing(4);

if let UpgradeState::Available(r) = &self.upgrade {
    let mut actions = row![].spacing(8).align_y(Center);
    // Hide "Update now" for Unknown installs (notify-only).
    if !matches!(self.upgrade_method, crate::upgrade::InstallMethod::Unknown) {
        actions = actions.push(modal_action("Update now", ModalBtn::Primary, Msg::StartUpdate));
    }
    actions = actions
        .push(modal_action("Skip this version", ModalBtn::Plain, Msg::SkipVersion))
        .push(link_open(&r.html_url, "Release notes")); // see step 2

    let action_row = container(actions).padding(Padding::from([4, 10]));
    updates_col = updates_col.push(action_row);
}

let updates_section = updates_col;
```

Use the exact `ModalBtn` variants and `modal_action`/`icon_btn` signatures present in `view.rs` (read them — the explore output confirms `modal_action(label, ModalBtn::Plain, msg)` and `ModalBtn::Primary`/`Plain`/`Default` exist; adjust the primary-button variant name to whatever the file defines).

- [ ] **Step 2: Release-notes link**

Grove may not have an existing "open URL" helper. Add the message-free approach: a `modal_action("Release notes", ModalBtn::Plain, Msg::OpenUrl(r.html_url.clone()))` is overkill. Instead reuse any existing external-open. Search `view.rs`/`update.rs` for an existing URL-open (`open::that`, `Command::new("open")`, or a `Msg::Open*`). If one exists, reuse it. If none exists, render the URL as plain `text(r.html_url.clone()).size(11).color(c::FG_MUTE())` (a non-clickable hint) — do **not** add an `open`/`webbrowser` crate (Global Constraints: no new deps beyond ureq+semver). Name the helper `link_open` accordingly or inline the text.

- [ ] **Step 3: Add the section to the modal body**

Change the final composition from:

```rust
let body = column![head, appearance, terminal, tools_section].spacing(16);
```

to:

```rust
let body = column![head, appearance, terminal, tools_section, updates_section].spacing(16);
```

- [ ] **Step 4: Add the cog badge**

Find where the settings cog is rendered in the appbar (search `view.rs` for `icon_btn("cog"` / `Msg::OpenSettings`). When `matches!(self.upgrade, UpgradeState::Available(_))`, overlay a small `dot(c::GREEN())` on the cog using a `stack![]` so the user sees "update available" without a popup. Concretely, wrap the existing cog button:

```rust
let cog = icon_btn("cog", Msg::OpenSettings);
let cog: Element<'_, Msg> = if matches!(self.upgrade, UpgradeState::Available(_)) {
    stack![
        cog,
        container(dot(c::GREEN()))
            .align_x(iced::alignment::Horizontal::Right)
            .align_y(iced::alignment::Vertical::Top)
            .width(Length::Fill)
            .height(Length::Fill),
    ]
    .into()
} else {
    cog
};
```

Respect `skipped_version`: the badge keys off `UpgradeState::Available`, which is only set when `update_available` returned true (which already excludes the skipped tag), so no extra check is needed.

- [ ] **Step 5: Verify it compiles**

Run: `cargo build`
Expected: clean build. Visually confirm later via manual run (Settings → Updates section renders; cog badge appears only when `Available`).

- [ ] **Step 6: Commit**

```bash
git add src/gui/view.rs
git commit -m "feat(upgrade): Settings Updates section and cog update badge"
```

---

### Task 6: Apply flow — `Modal::Updating` progress modal, background apply thread, restart

**Files:**
- Modify: `src/app.rs:145-238` — add a `Modal::Updating` variant.
- Modify: `src/gui/update.rs` — `Msg::StartUpdate` spawns the apply thread (writing stages into `upgrade_progress`); the `Tick` arm polls `upgrade_progress` and emits `Msg::UpdateStageChanged`; `Msg::UpdateStageChanged`/`Msg::UpdateFinished`/`Msg::RestartApp` handlers.
- Modify: `src/gui/view.rs` — render `Modal::Updating` via `modal_layer()` (`src/gui/view.rs:1419-1505`), and add a `updating_modal()` view fn following `modal_panel` style.

**Interfaces:**
- Consumes: `crate::upgrade::{apply, Stage}`, `Grove.upgrade_method`, `Grove.upgrade`, `Grove.upgrade_progress`, the `Modal` enum and `modal_layer` dispatch.
- Produces: `Modal::Updating` variant; the streaming apply.

- [ ] **Step 1: Add the `Modal::Updating` variant**

In `src/app.rs` `Modal` enum, add:

```rust
    /// Apply-in-progress overlay; mirrors the one-deep modal pattern.
    Updating,
```

- [ ] **Step 2: `Msg::StartUpdate` — spawn the apply thread**

In `update()`, add:

```rust
Msg::StartUpdate => {
    let UpgradeState::Available(release) = self.upgrade.clone() else {
        return Task::none();
    };
    let method = self.upgrade_method;
    self.upgrade = UpgradeState::Updating(crate::upgrade::Stage::Downloading);
    self.app.modal = Modal::Updating;

    let progress = self.upgrade_progress.clone();
    let progress_done = self.upgrade_progress.clone();
    // Spawn a real OS thread: apply blocks (clone/compile or dmg copy) for a long
    // time and must not occupy the iced/tokio executor. Stages land in the shared
    // mutex, which the Tick handler drains into Msg::UpdateStageChanged.
    std::thread::spawn(move || {
        let cb = move |stage: crate::upgrade::Stage| {
            if let Ok(mut g) = progress.lock() {
                *g = Some(stage);
            }
        };
        let result = crate::upgrade::apply(method, &release, &cb).map_err(|e| e.to_string());
        // Push terminal result through the same channel as a sentinel via a second field?
        // Simpler: stash result so Tick can emit UpdateFinished.
        if let Ok(mut g) = progress_done.lock() {
            // Done stage already set by apply on success; on error we record it below.
            if result.is_err() {
                *g = Some(crate::upgrade::Stage::Done); // unblock Tick to read the error
            }
        }
        // Store the terminal result for Tick to pick up.
        APPLY_RESULT.with(|_| {}); // placeholder — see note
    });
    return Task::none();
}
```

**Implementation note (resolve before coding):** the closure-only progress channel cannot also carry the final `Result`. Use one shared handle that carries **both** progress and completion. Replace `upgrade_progress: Arc<Mutex<Option<Stage>>>` with `Arc<Mutex<UpgradeProgress>>` where:

```rust
#[derive(Default)]
pub struct UpgradeProgress {
    pub stage: Option<crate::upgrade::Stage>,
    pub finished: Option<Result<(), String>>,
}
```

The apply thread writes `stage` on each callback and sets `finished = Some(result)` at the end. The `Tick` handler reads the handle: if `stage` changed, emit `Msg::UpdateStageChanged(stage)`; if `finished` is `Some`, emit `Msg::UpdateFinished(result)` and clear it. Update Task 4's field type accordingly (this task owns the `UpgradeProgress` struct; adjust the Task-4 `upgrade_progress` initializer to `Arc::new(Mutex::new(UpgradeProgress::default()))`). Remove the placeholder `APPLY_RESULT`/`progress_done` sketch above — it was illustrative.

Concrete thread body:

```rust
let handle = self.upgrade_progress.clone();
std::thread::spawn(move || {
    let cb_handle = handle.clone();
    let cb = move |stage: crate::upgrade::Stage| {
        if let Ok(mut g) = cb_handle.lock() {
            g.stage = Some(stage);
        }
    };
    let result = crate::upgrade::apply(method, &release, &cb).map_err(|e| e.to_string());
    if let Ok(mut g) = handle.lock() {
        g.finished = Some(result);
    }
});
```

- [ ] **Step 3: Drain progress in the `Tick` arm**

In the `Msg::Tick` arm, after the periodic-check gate from Task 4, drain the handle:

```rust
// Drain apply progress (set by the background apply thread).
let drained = {
    if let Ok(mut g) = self.upgrade_progress.lock() {
        let stage = g.stage.take();
        let finished = g.finished.take();
        (stage, finished)
    } else {
        (None, None)
    }
};
if let Some(stage) = drained.0 {
    self.upgrade = UpgradeState::Updating(stage);
}
if let Some(result) = drained.1 {
    self.upgrade = match result {
        Ok(()) => UpgradeState::Updated,
        Err(e) => UpgradeState::UpdateFailed(e),
    };
}
```

(If the `Tick` arm returns early in some branches, ensure this drain runs every tick — put it before any early return that the periodic check might trigger, or fold the periodic check after it.)

- [ ] **Step 4: `Msg::RestartApp` handler**

```rust
Msg::RestartApp => {
    // Re-exec the (now-replaced) binary, then exit. No auto-restart of PTYs.
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::process::Command::new(exe).spawn();
    }
    std::process::exit(0);
}
```

(`Msg::UpdateStageChanged`/`Msg::UpdateFinished` declared in Task 4 are now driven directly from the `Tick` drain; if you prefer explicit messages over the drain-in-Tick approach, emit them from Tick instead. Either is acceptable — keep one path. Recommended: the drain-in-Tick path above; in that case `UpdateStageChanged`/`UpdateFinished` may be unused — remove them from the `Msg` enum to avoid dead variants, and note this in the implementer report so the reviewer knows Task 4's variants were trimmed.)

- [ ] **Step 5: Render `Modal::Updating`**

In `modal_layer()` (`src/gui/view.rs:1419-1505`), add to the match:

```rust
Modal::Updating => self.updating_modal(),
```

Add the view fn (follow `modal_panel(body, width, accent)` used by `settings_modal`):

```rust
fn updating_modal(&self) -> Element<'_, Msg> {
    let header = text("Updating Grove").size(13).color(c::MAGENTA());

    let body: Element<'_, Msg> = match &self.upgrade {
        UpgradeState::Updating(stage) => {
            let label = match stage {
                crate::upgrade::Stage::Downloading => "Downloading…",
                crate::upgrade::Stage::Building => "Building…",
                crate::upgrade::Stage::Installing => "Installing…",
                crate::upgrade::Stage::Done => "Finishing…",
            };
            row![
                spinner(16.0, c::FG_DIM(), self.tick),
                Space::with_width(10),
                text(label).size(12).color(c::FG()),
            ]
            .align_y(Center)
            .into()
        }
        UpgradeState::Updated => column![
            text("Update installed — restart grove to apply")
                .size(12)
                .color(c::FG()),
            Space::with_height(10),
            row![
                modal_action("Restart", ModalBtn::Primary, Msg::RestartApp),
                Space::with_width(8),
                modal_action("Later", ModalBtn::Plain, Msg::ModalCancel),
            ]
            .align_y(Center),
        ]
        .into(),
        UpgradeState::UpdateFailed(e) => column![
            text("Update failed").size(12).color(c::FG()),
            Space::with_height(6),
            text(e.clone()).size(11).color(c::FG_MUTE()),
            Space::with_height(10),
            modal_action("Close", ModalBtn::Plain, Msg::ModalCancel),
        ]
        .into(),
        _ => text("Updating…").size(12).color(c::FG_DIM()).into(),
    };

    let content = column![header, Space::with_height(12), body].spacing(0);
    modal_panel(content.into(), 420.0, c::MAGENTA())
}
```

Match the exact `modal_panel`/`modal_action`/`ModalBtn` signatures in `view.rs`. The `Msg::ModalCancel` close path already sets `Modal::None` for other modals — confirm it resets cleanly here (it should leave `self.upgrade` as `Updated`/`UpdateFailed`; that's fine — the badge logic only reacts to `Available`).

- [ ] **Step 6: Allow closing the modal**

In the Escape-handling for modals (`src/gui/update.rs:1384-1387` shows the `Modal::Settings` Escape arm), add `Modal::Updating` so Escape closes it **only when not actively updating** (avoid letting the user dismiss mid-build and lose the progress view — but the thread keeps running regardless). Allow Escape when `Updated`/`UpdateFailed`, ignore it when `Updating`:

```rust
Modal::Updating => {
    if matches!(key, Key::Named(Named::Escape))
        && !matches!(self.upgrade, UpgradeState::Updating(_))
    {
        self.app.modal = Modal::None;
    }
}
```

- [ ] **Step 7: Verify it compiles**

Run: `cargo build && cargo test`
Expected: clean build; all existing + Task 1/Task 3 tests pass (`test result: ok.`).

- [ ] **Step 8: Commit**

```bash
git add src/app.rs src/gui/update.rs src/gui/view.rs src/gui/state.rs
git commit -m "feat(upgrade): apply flow with progress modal and restart prompt"
```

---

## Self-Review (author checklist — completed)

**Spec coverage:**
- Module boundary (`detect`/`latest`/`apply`, no iced dep) → Tasks 1, 2. ✓
- New deps `ureq`(rustls)/`semver` → Task 1. ✓
- Install-method detection (Source/Dmg/Deb/Unknown; Deb via source) → Task 1 (`classify_path`) + Task 2 (`apply` routing). ✓
- Version check (`/latest`, strip `v`, semver compare, skip logic) → Task 1 (`parse_release`, `update_available`). ✓
- Persistence (`last_update_check`, `skipped_version`, serde default) → Task 3. ✓
- Triggers (launch ~3s, manual, periodic 24h) → Task 4. ✓
- State & messages (`UpgradeState`, Msg variants) → Task 4. ✓
- UI: Settings Updates section, cog badge, progress modal + Restart → Tasks 5, 6. ✓
- Non-fatal failures (manual surfaces error, launch/periodic silent) → Task 4 handler logs + sets `Error` (Settings shows it; no popup elsewhere). ✓
- No auto-restart → Task 6 (`Restart` button only). ✓
- Glyph rule (reuse SVG icons) → Tasks 5, 6 use `spinner`/`dot`/`icon`, no new Unicode. ✓

**Type consistency:** `UpgradeState`, `Stage`, `Release`, `InstallMethod`, `UpgradeProgress`, and all `Msg` variants are referenced with matching names/types across tasks. Task 4 introduces `upgrade_progress` as `Arc<Mutex<Option<Stage>>>`; **Task 6 supersedes it with `Arc<Mutex<UpgradeProgress>>`** — the implementer of Task 6 must update the field type and its initializer (called out explicitly in Task 6 Step 2).

**Known adaptation points (read the file, match exact signatures):** `modal_action`/`ModalBtn` variant names, `modal_panel` arity, `icon_btn`, the appbar cog location, the initial-`Task` boot site, and the existing logging macro. These are anchored with file:line in each task.
