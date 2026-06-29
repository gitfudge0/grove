//! In-place self-update for Grove. Pure logic with no `iced` dependency so it
//! unit-tests standalone; the gui layer orchestrates it. Named `upgrade` (not
//! `update`) to avoid confusion with `gui::update`, the iced message loop.

use anyhow::{anyhow, Context, Result};
use std::path::Path;
use std::process::Command;
use std::time::Duration;

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
/// Blocks the calling thread — call from a background thread, never the UI thread.
pub fn latest() -> Result<Release> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(30))
        .build();
    let body = agent
        .get("https://api.github.com/repos/gitfudge0/grove/releases/latest")
        .set("User-Agent", "grove")
        .set("Accept", "application/vnd.github+json")
        .call()
        .context("github releases request failed")?
        .into_string()
        .context("read github response body")?;
    parse_release(&body)
}

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
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(10))
            .timeout_read(Duration::from_secs(300))
            .build();
        let mut reader = agent
            .get(url)
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
        let app_in_dmg_str = app_in_dmg.to_string_lossy().into_owned();
        run(
            Command::new("ditto").args([app_in_dmg_str.as_str(), app_path.as_str()]),
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
    let _ = std::fs::remove_dir(&mnt);
    result
}

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
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(30))
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
}
