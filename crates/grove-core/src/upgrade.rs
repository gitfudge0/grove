//! In-place self-update for Grove. Pure logic with no `iced` dependency so it
//! unit-tests standalone; the gui layer orchestrates it. Named `upgrade` (not
//! `update`) to avoid confusion with `gui::update`, the iced message loop.

use fs_err as fs;
use std::path::Path;
use std::process::Command;
use std::time::Duration;
use thiserror::Error;

/// Everything the self-update path can fail with. Mirrors `git::GitError` /
/// `storage::StoreError`: each variant is one real failure site below, and
/// the `Display` text matches what the previous `anyhow` context produced, so
/// the strings surfaced in the upgrade modal are unchanged.
#[derive(Debug, Error)]
pub enum UpgradeError {
    #[error("parse release json")]
    ParseReleaseJson(#[source] serde_json::Error),
    #[error("release json missing tag_name")]
    MissingTagName,
    #[error("parse semver from tag {tag}")]
    Semver {
        tag: String,
        #[source]
        source: semver::Error,
    },
    #[error("github releases request failed")]
    ReleasesRequest(#[source] Box<ureq::Error>),
    #[error("read github response body")]
    ReadReleaseBody(#[source] std::io::Error),
    #[error("releases json is not an array")]
    ReleasesNotArray,
    #[error("parse releases json")]
    ParseReleasesJson(#[source] serde_json::Error),
    #[error("github releases list request failed")]
    ReleasesListRequest(#[source] Box<ureq::Error>),
    #[error("read github releases list body")]
    ReadReleasesListBody(#[source] std::io::Error),
    #[error("unknown install method — cannot self-update")]
    UnknownInstallMethod,
    #[error(
        "git and cargo are required to update from source. Reinstall with:\n  \
         curl -fsSL https://raw.githubusercontent.com/gitfudge0/grove/main/install.sh | sh"
    )]
    MissingBuildTools,
    #[error("create temp dir")]
    CreateTempDir(#[source] std::io::Error),
    /// The release names an immutable commit, but the clone landed elsewhere
    /// — a tag was moved between the API response and the clone.
    #[error(
        "release {tag} points at commit {expected} but the clone is at {actual} — refusing to build"
    )]
    CommitMismatch {
        tag: String,
        expected: String,
        actual: String,
    },
    #[error("release asset url is not https: {0}")]
    AssetUrlNotHttps(String),
    #[error("refusing to download release asset from {0}")]
    AssetHostNotAllowed(String),
    #[error("download release asset")]
    DownloadAsset(#[source] Box<ureq::Error>),
    #[error("redirect without a Location header")]
    RedirectWithoutLocation,
    #[error("read release asset body")]
    ReadAssetBody(#[source] std::io::Error),
    #[error("too many redirects downloading release asset")]
    TooManyRedirects,
    #[error("empty .sha256 file")]
    EmptyChecksumFile,
    #[error("malformed sha256 digest in checksum file")]
    MalformedChecksum,
    #[error("release has no .dmg asset")]
    NoDmgAsset,
    #[error("current_exe")]
    CurrentExe(#[source] std::io::Error),
    #[error("could not resolve running .app bundle from {0}")]
    NoAppBundle(String),
    #[error("download dmg")]
    DownloadDmg(#[source] Box<UpgradeError>),
    #[error("download dmg checksum")]
    DownloadDmgChecksum(#[source] Box<UpgradeError>),
    #[error("downloaded dmg does not match its published sha256 — refusing to install")]
    ChecksumMismatch,
    #[error("write dmg file")]
    WriteDmg(#[source] std::io::Error),
    #[error("read mounted volume")]
    ReadMountedVolume(#[source] std::io::Error),
    #[error("no .app found in mounted dmg")]
    NoAppInDmg,
    /// A subprocess could not be spawned at all. `what` names the command
    /// (e.g. `git clone`).
    #[error("failed to spawn {what}")]
    Spawn {
        what: String,
        #[source]
        source: std::io::Error,
    },
    /// A subprocess ran but exited non-zero; `stderr` is its trimmed output.
    #[error("{what} failed: {stderr}")]
    Command { what: String, stderr: String },
}

/// Shorthand for this module's fallible functions.
pub type Result<T, E = UpgradeError> = std::result::Result<T, E>;

/// How the running binary was installed. Determines the apply strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallMethod {
    /// `cargo install --path .` into `~/.cargo/bin` (a manual/legacy install). Rebuild from source.
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
    /// `browser_download_url` of the sibling `<dmg name>.sha256` asset, if the
    /// release publishes one. `None` means the release has no checksum to
    /// verify against (see `apply_dmg`).
    pub dmg_sha256_url: Option<String>,
    /// The release's `target_commitish` verbatim. A 40-char hex SHA is
    /// verified against the cloned HEAD in `apply_source`; a branch name is
    /// not (see there).
    pub target_commitish: String,
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
            let canonical = fs::canonicalize(&p).unwrap_or(p);
            classify_path(&canonical, std::env::consts::OS)
        }
        Err(_) => InstallMethod::Unknown,
    }
}

/// Parse a GitHub `/releases/latest` JSON body into a `Release`.
fn parse_release(json: &str) -> Result<Release> {
    let v: serde_json::Value =
        serde_json::from_str(json).map_err(UpgradeError::ParseReleaseJson)?;
    let tag = v
        .get("tag_name")
        .and_then(|t| t.as_str())
        .ok_or(UpgradeError::MissingTagName)?
        .to_string();
    let version = semver::Version::parse(tag.trim_start_matches('v')).map_err(|source| {
        UpgradeError::Semver {
            tag: tag.clone(),
            source,
        }
    })?;
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
    let target_commitish = v
        .get("target_commitish")
        .and_then(|t| t.as_str())
        .unwrap_or("")
        .to_string();
    let empty = Vec::new();
    let assets = v.get("assets").and_then(|a| a.as_array()).unwrap_or(&empty);
    let url_of = |want: &str| -> Option<String> {
        assets.iter().find_map(|asset| {
            let name = asset.get("name").and_then(|n| n.as_str())?;
            (name == want)
                .then(|| asset.get("browser_download_url").and_then(|u| u.as_str()))
                .flatten()
                .map(String::from)
        })
    };
    let dmg_name = assets.iter().find_map(|asset| {
        let name = asset.get("name").and_then(|n| n.as_str())?;
        name.ends_with(".dmg").then(|| name.to_string())
    });
    let dmg_url = dmg_name.as_deref().and_then(url_of);
    // A checksum asset only counts when it sits beside the exact file we're
    // about to download — `<dmg name>.sha256`.
    let dmg_sha256_url = dmg_name
        .as_deref()
        .and_then(|n| url_of(&format!("{n}.sha256")));
    Ok(Release {
        version,
        tag,
        html_url,
        body,
        dmg_url,
        dmg_sha256_url,
        target_commitish,
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
        .map_err(|e| {
            tracing::warn!(error = %e, "upgrade: github releases request failed");
            UpgradeError::ReleasesRequest(Box::new(e))
        })?
        .into_string()
        .map_err(UpgradeError::ReadReleaseBody)?;
    let release = parse_release(&body);
    match &release {
        Ok(r) => {
            tracing::info!(current = env!("CARGO_PKG_VERSION"), latest = %r.version, "upgrade: version check result");
        }
        Err(e) => tracing::warn!(error = %e, "upgrade: failed to parse latest release"),
    }
    release
}

/// Perform the upgrade for the detected install method, reporting stage
/// transitions through `progress`. Runs on a background thread (blocking).
pub fn apply(
    method: InstallMethod,
    release: &Release,
    progress: &(dyn Fn(Stage) + Send + Sync),
) -> Result<()> {
    tracing::info!(method = ?method, tag = %release.tag, "upgrade: applying update");
    let result = match method {
        InstallMethod::Source | InstallMethod::Deb => apply_source(release, progress),
        InstallMethod::Dmg => apply_dmg(release, progress),
        InstallMethod::Unknown => Err(UpgradeError::UnknownInstallMethod),
    };
    match &result {
        Ok(()) => tracing::info!(tag = %release.tag, "upgrade: update applied successfully"),
        Err(e) => tracing::warn!(error = %e, tag = %release.tag, "upgrade: update failed"),
    }
    result
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
        return Err(UpgradeError::MissingBuildTools);
    }

    // A fresh 0700 directory with an unpredictable name, created exclusively:
    // a predictable `/tmp/grove-upgrade-{pid}` path is pre-creatable (and
    // symlink-swappable) by any other local user. `TempDir` also removes the
    // directory on drop, so we never `remove_dir_all` a path we didn't make.
    let tmp_dir = tempfile::Builder::new()
        .prefix("grove-upgrade-")
        .tempdir()
        .map_err(UpgradeError::CreateTempDir)?;
    let tmp = tmp_dir.path();
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

        // A tag is mutable on the server: whoever can move `release.tag` can
        // point this clone at code the release JSON never described. When the
        // release names an immutable commit, pin to it.
        if let Some(expected) = commit_to_verify(&release.target_commitish) {
            let head = capture(
                Command::new("git").args(["-C", &tmp.to_string_lossy(), "rev-parse", "HEAD"]),
                "git rev-parse HEAD",
            )?;
            let head = head.trim();
            if !head.eq_ignore_ascii_case(expected) {
                return Err(UpgradeError::CommitMismatch {
                    tag: release.tag.clone(),
                    expected: expected.to_string(),
                    actual: head.to_string(),
                });
            }
        }

        progress(Stage::Building);
        run(
            Command::new("cargo").args(["install", "--path", &tmp.to_string_lossy(), "--force"]),
            "cargo install",
        )?;

        progress(Stage::Done);
        Ok(())
    })();
    // `tmp_dir` removes the directory tree on drop.
    result
}

/// The commit a release pins to, when `target_commitish` is one. GitHub sets
/// this field to a branch name (`main`) for releases cut from a branch, which
/// says nothing about which commit the tag resolves to — there is nothing to
/// verify against, so those are skipped rather than failed.
fn commit_to_verify(target_commitish: &str) -> Option<&str> {
    let s = target_commitish.trim();
    (s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit())).then_some(s)
}

/// Hosts we accept a release asset download from. GitHub serves release assets
/// from `github.com`, which redirects to its S3-backed object host; anything
/// else means the release JSON was tampered with (or points somewhere we don't
/// trust) and must not be fetched, let alone installed.
const ALLOWED_ASSET_HOSTS: [&str; 2] = ["github.com", "objects.githubusercontent.com"];

/// Host component of an `https://` URL, lowercased and stripped of any
/// userinfo and port. `None` for anything that isn't plain `https://`.
fn https_host(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    // `user:pass@host` — the host is what follows the last '@'.
    let host = authority.rsplit('@').next()?;
    let host = host.split(':').next()?;
    if host.is_empty() {
        None
    } else {
        Some(host.to_ascii_lowercase())
    }
}

/// Reject an asset URL that isn't https on a host we trust.
fn check_asset_url(url: &str) -> Result<()> {
    let host = https_host(url).ok_or_else(|| UpgradeError::AssetUrlNotHttps(url.to_string()))?;
    if !ALLOWED_ASSET_HOSTS.contains(&host.as_str()) {
        return Err(UpgradeError::AssetHostNotAllowed(host));
    }
    Ok(())
}

/// Maximum redirect hops we follow ourselves. GitHub's asset URLs take one
/// (github.com → objects.githubusercontent.com); the slack is for extra hops
/// on their side, not for chasing a chain somewhere else.
const MAX_REDIRECTS: usize = 5;

/// Fetch `url` into memory, following redirects MANUALLY so every hop is
/// re-checked against the host allowlist. `ureq`'s own redirect follower does
/// not consult `check_asset_url`, so a `Location` header pointing anywhere at
/// all would be fetched — the allowlist would only ever have covered the first
/// URL, which is the one we already trusted least.
fn download_checked(url: &str, timeout_read: Duration) -> Result<Vec<u8>> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(timeout_read)
        .redirects(0)
        .build();
    let mut url = url.to_string();
    for _ in 0..=MAX_REDIRECTS {
        check_asset_url(&url)?;
        let resp = agent
            .get(&url)
            .set("User-Agent", "grove")
            .call()
            .map_err(|e| {
                tracing::warn!(error = %e, "upgrade: asset download failed");
                UpgradeError::DownloadAsset(Box::new(e))
            })?;
        if (300..400).contains(&resp.status()) {
            let location = resp
                .header("Location")
                .ok_or(UpgradeError::RedirectWithoutLocation)?;
            url = resolve_location(&url, location);
            continue;
        }
        let mut buf = Vec::new();
        std::io::copy(&mut resp.into_reader(), &mut buf).map_err(UpgradeError::ReadAssetBody)?;
        return Ok(buf);
    }
    Err(UpgradeError::TooManyRedirects)
}

/// Resolve a `Location` header against the URL it came from. Only absolute
/// URLs and root-relative paths occur in practice; anything else is returned
/// verbatim so `check_asset_url` rejects it on the next hop.
fn resolve_location(base: &str, location: &str) -> String {
    if location.contains("://") {
        return location.to_string();
    }
    if let Some(rest) = location.strip_prefix('/') {
        if let Some(host) = https_host(base) {
            return format!("https://{host}/{rest}");
        }
    }
    location.to_string()
}

/// Lowercase hex SHA-256 of `bytes`.
fn sha256_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    let digest = ring::digest::digest(&ring::digest::SHA256, bytes);
    digest.as_ref().iter().fold(String::new(), |mut out, b| {
        let _ = write!(out, "{b:02x}");
        out
    })
}

/// Pull the digest out of a `.sha256` file: either a bare hex digest or the
/// `sha256sum` format (`<digest>  <filename>`).
fn parse_sha256_file(raw: &str) -> Result<String> {
    let token = raw
        .split_whitespace()
        .next()
        .ok_or(UpgradeError::EmptyChecksumFile)?;
    if token.len() != 64 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(UpgradeError::MalformedChecksum);
    }
    Ok(token.to_ascii_lowercase())
}

/// Download the `.dmg`, mount it, copy the `.app` over the running bundle, detach.
fn apply_dmg(release: &Release, progress: &(dyn Fn(Stage) + Send + Sync)) -> Result<()> {
    let url = release.dmg_url.as_deref().ok_or(UpgradeError::NoDmgAsset)?;
    // TODO: verify published checksum once releases include one. Until they
    // publish a sibling `<asset>.sha256`, the download below is verified only
    // by TLS to an allowlisted host; when one IS published we verify it.
    check_asset_url(url)?;

    // Resolve the running .app bundle: strip "/Contents/MacOS/<bin>".
    let exe = std::env::current_exe().map_err(UpgradeError::CurrentExe)?;
    let exe = fs::canonicalize(&exe).unwrap_or(exe);
    let exe_str = exe.to_string_lossy();
    let app_path = exe_str
        .split("/Contents/MacOS/")
        .next()
        .filter(|p| p.ends_with(".app"))
        .ok_or_else(|| UpgradeError::NoAppBundle(exe_str.to_string()))?
        .to_string();

    // Same reasoning as `apply_source`: an unpredictable 0700 directory we own,
    // holding both the downloaded image and the mount point.
    let tmp_dir = tempfile::Builder::new()
        .prefix("grove-upgrade-")
        .tempdir()
        .map_err(UpgradeError::CreateTempDir)?;
    let dmg_path = tmp_dir.path().join("grove.dmg");
    let mnt = tmp_dir.path().join("mnt");

    let result = (|| -> Result<()> {
        progress(Stage::Downloading);
        let payload = download_checked(url, Duration::from_secs(300))
            .map_err(|e| UpgradeError::DownloadDmg(Box::new(e)))?;

        // Verify against the published checksum when the release has one. The
        // digest is fetched through the same allowlisted, redirect-checked
        // path as the payload, so it is no weaker a link than the download.
        if let Some(sha_url) = release.dmg_sha256_url.as_deref() {
            let raw = download_checked(sha_url, Duration::from_secs(30))
                .map_err(|e| UpgradeError::DownloadDmgChecksum(Box::new(e)))?;
            let expected = parse_sha256_file(&String::from_utf8_lossy(&raw))?;
            let actual = sha256_hex(&payload);
            if actual != expected {
                tracing::warn!(expected, actual, "upgrade: dmg checksum mismatch");
                return Err(UpgradeError::ChecksumMismatch);
            }
            tracing::info!("upgrade: dmg checksum verified");
        } else {
            tracing::warn!(
                "upgrade: release publishes no .sha256 for the dmg — installing unverified"
            );
        }

        fs::write(&dmg_path, &payload).map_err(|e| {
            tracing::warn!(error = %e, path = %dmg_path.display(), "upgrade: write dmg file failed");
            UpgradeError::WriteDmg(e)
        })?;

        progress(Stage::Installing);
        fs::create_dir_all(&mnt).ok();
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
        let app_in_dmg = fs::read_dir(&mnt)
            .map_err(UpgradeError::ReadMountedVolume)?
            .filter_map(std::result::Result::ok)
            .map(|e| e.path())
            .find(|p| p.extension().is_some_and(|x| x == "app"))
            .ok_or(UpgradeError::NoAppInDmg)?;

        // Replace the running bundle. `ditto` preserves macOS metadata.
        let app_in_dmg_str = app_in_dmg.to_string_lossy().into_owned();
        run(
            Command::new("ditto").args([app_in_dmg_str.as_str(), app_path.as_str()]),
            "ditto copy",
        )?;

        progress(Stage::Done);
        Ok(())
    })();

    // Always detach, even on error; `tmp_dir` removes the rest on drop.
    let _ = run(
        Command::new("hdiutil").args(["detach", &mnt.to_string_lossy()]),
        "hdiutil detach",
    );
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
    let v: serde_json::Value =
        serde_json::from_str(json).map_err(UpgradeError::ParseReleasesJson)?;
    let arr = v.as_array().ok_or(UpgradeError::ReleasesNotArray)?;
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
        out.push(ReleaseNote {
            tag,
            name,
            date,
            body,
        });
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
    let url = format!("https://api.github.com/repos/gitfudge0/grove/releases?per_page={limit}");
    let body = agent
        .get(&url)
        .set("User-Agent", "grove")
        .set("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| UpgradeError::ReleasesListRequest(Box::new(e)))?
        .into_string()
        .map_err(UpgradeError::ReadReleasesListBody)?;
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
    while lines.first().is_some_and(|l| l.trim().is_empty()) {
        lines.remove(0);
    }
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
    lines.join("\n")
}

/// Like `run`, but returns the command's stdout as a `String`.
fn capture(cmd: &mut Command, what: &str) -> Result<String> {
    let out = cmd.output().map_err(|source| UpgradeError::Spawn {
        what: what.to_string(),
        source,
    })?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        tracing::warn!(status = ?out.status, stderr = %stderr, what, "upgrade: command failed");
        return Err(UpgradeError::Command {
            what: what.to_string(),
            stderr: stderr.trim().to_string(),
        });
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Run a command, mapping non-zero exit (and spawn failure) to a contextual error.
fn run(cmd: &mut Command, what: &str) -> Result<()> {
    let out = cmd.output().map_err(|source| {
        tracing::warn!(error = %source, what, "upgrade: failed to spawn command");
        UpgradeError::Spawn {
            what: what.to_string(),
            source,
        }
    })?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        tracing::warn!(status = ?out.status, stderr = %stderr, what, "upgrade: command failed");
        return Err(UpgradeError::Command {
            what: what.to_string(),
            stderr: stderr.trim().to_string(),
        });
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
            dmg_sha256_url: None,
            target_commitish: String::new(),
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
        assert!(!update_available(
            "0.24.0",
            &release("v0.25.0"),
            Some("v0.25.0")
        ));
    }

    #[test]
    fn newer_than_skipped_surfaces_again() {
        // User skipped v0.25.0; v0.26.0 must still surface.
        assert!(update_available(
            "0.24.0",
            &release("v0.26.0"),
            Some("v0.25.0")
        ));
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
    fn asset_url_allows_github_hosts() {
        assert!(check_asset_url(
            "https://github.com/gitfudge0/grove/releases/download/v1/Grove.dmg"
        )
        .is_ok());
        assert!(check_asset_url("https://objects.githubusercontent.com/x/Grove.dmg").is_ok());
    }

    #[test]
    fn asset_url_rejects_other_hosts_and_schemes() {
        assert!(check_asset_url("https://evil.example/Grove.dmg").is_err());
        assert!(check_asset_url("http://github.com/Grove.dmg").is_err());
        // Userinfo must not be mistaken for the host.
        assert!(check_asset_url("https://github.com@evil.example/Grove.dmg").is_err());
        assert!(check_asset_url("not a url").is_err());
    }

    #[test]
    fn parse_release_picks_up_sibling_sha256_asset() {
        let json = r#"{
            "tag_name": "v0.25.0",
            "target_commitish": "main",
            "assets": [
                {"name": "Grove.dmg", "browser_download_url": "https://x/Grove.dmg"},
                {"name": "Grove.dmg.sha256", "browser_download_url": "https://x/Grove.dmg.sha256"}
            ]
        }"#;
        let r = parse_release(json).unwrap();
        assert_eq!(
            r.dmg_sha256_url.as_deref(),
            Some("https://x/Grove.dmg.sha256")
        );
        // A checksum for a different asset must not be picked up.
        let json = r#"{
            "tag_name": "v0.25.0",
            "assets": [
                {"name": "Grove.dmg", "browser_download_url": "https://x/Grove.dmg"},
                {"name": "grove.deb.sha256", "browser_download_url": "https://x/grove.deb.sha256"}
            ]
        }"#;
        assert!(parse_release(json).unwrap().dmg_sha256_url.is_none());
    }

    #[test]
    fn commit_to_verify_only_accepts_a_full_sha() {
        let sha = "0123456789abcdef0123456789abcdef01234567";
        assert_eq!(commit_to_verify(sha), Some(sha));
        assert_eq!(commit_to_verify("main"), None);
        assert_eq!(commit_to_verify(""), None);
        // Short SHAs and non-hex are not commits we can pin to.
        assert_eq!(commit_to_verify("0123456"), None);
        assert_eq!(
            commit_to_verify("z123456789abcdef0123456789abcdef01234567"),
            None
        );
    }

    #[test]
    fn sha256_hex_matches_known_vector() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn parse_sha256_file_accepts_both_shapes() {
        let d = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert_eq!(parse_sha256_file(d).unwrap(), d);
        assert_eq!(parse_sha256_file(&format!("{d}  Grove.dmg\n")).unwrap(), d);
        assert_eq!(parse_sha256_file(&d.to_uppercase()).unwrap(), d);
        assert!(parse_sha256_file("").is_err());
        assert!(parse_sha256_file("deadbeef").is_err());
    }

    #[test]
    fn resolve_location_handles_absolute_and_root_relative() {
        assert_eq!(
            resolve_location(
                "https://github.com/a/b",
                "https://objects.githubusercontent.com/x"
            ),
            "https://objects.githubusercontent.com/x"
        );
        assert_eq!(
            resolve_location("https://github.com/a/b", "/c/d"),
            "https://github.com/c/d"
        );
        // A relative hop we don't resolve stays as-is, so the allowlist check
        // on the next iteration rejects it.
        assert!(check_asset_url(&resolve_location("https://github.com/a/b", "c/d")).is_err());
    }

    #[test]
    fn https_host_strips_port_and_case() {
        assert_eq!(
            https_host("https://GitHub.com:443/x"),
            Some("github.com".into())
        );
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
