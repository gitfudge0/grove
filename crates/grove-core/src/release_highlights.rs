//! Curated, release-specific highlights and the once-per-installed-version policy.
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use thiserror::Error;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema_version: u32,
    pub releases: Vec<ReleaseHighlights>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseHighlights {
    pub version: String,
    pub enabled: bool,
    pub title: String,
    pub slides: Vec<HighlightSlide>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HighlightSlide {
    pub id: String,
    pub title: String,
    pub description: String,
    pub media: HighlightMedia,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HighlightMedia {
    pub kind: MediaKind,
    pub src: String,
    pub alt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub poster: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub captions: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame: Option<ImageFrame>,
}
/// Authored screenshot framing; positions and highlight bounds use normalized coordinates.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageFrame {
    pub zoom: f32,
    pub aspect_ratio: f32,
    pub position_x: f32,
    pub position_y: f32,
    /// Optional [left, top, width, height] in the original image.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub highlight: Option<[f32; 4]>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MediaKind {
    Image,
    Gif,
    Video,
}
#[derive(Debug, Error)]
pub enum ManifestError {
    #[error("invalid highlights JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid highlights manifest: {0}")]
    Invalid(String),
}
fn invalid(message: &str) -> ManifestError {
    ManifestError::Invalid(message.to_owned())
}
fn normalized(version: &str) -> &str {
    version.strip_prefix('v').unwrap_or(version)
}
fn text_valid(value: &str, limit: usize) -> bool {
    !value.trim().is_empty() && value.len() <= limit && !value.chars().any(char::is_control)
}
fn path_valid(value: &str) -> bool {
    text_valid(value, 512)
        && value.starts_with("highlights/")
        && !value.contains(['\\', ':', '?', '#', '%'])
        && value
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}
impl Manifest {
    pub fn parse(bytes: &[u8]) -> Result<Self, ManifestError> {
        if bytes.len() > 512 * 1024 {
            return Err(invalid("manifest exceeds 512 KiB"));
        }
        let manifest: Self = serde_json::from_slice(bytes)?;
        if manifest.schema_version != 1 || manifest.releases.len() > 128 {
            return Err(invalid("unsupported schema or too many releases"));
        }
        let mut versions = HashSet::new();
        for release in &manifest.releases {
            let version = normalized(&release.version);
            if release.version.len() > 128
                || semver::Version::parse(version).is_err()
                || !versions.insert(version)
            {
                return Err(invalid("invalid or duplicate release version"));
            }
            if !text_valid(&release.title, 160)
                || release.slides.len() > 8
                || (release.enabled && release.slides.is_empty())
            {
                return Err(invalid("invalid release title or slide count"));
            }
            let mut ids = HashSet::new();
            for slide in &release.slides {
                if !text_valid(&slide.id, 80)
                    || !ids.insert(&slide.id)
                    || !text_valid(&slide.title, 160)
                    || !text_valid(&slide.description, 1200)
                    || !text_valid(&slide.media.alt, 1200)
                {
                    return Err(invalid("invalid slide content or duplicate ID"));
                }
                let media = &slide.media;
                if !path_valid(&media.src)
                    || media.poster.as_deref().is_some_and(|p| !path_valid(p))
                    || media.captions.as_deref().is_some_and(|p| !path_valid(p))
                {
                    return Err(invalid("media paths must be relative asset paths"));
                }
                if let Some(frame) = &media.frame {
                    let normalized = |v: f32| v.is_finite() && (0.0..=1.0).contains(&v);
                    if media.kind == MediaKind::Video
                        || !frame.zoom.is_finite()
                        || !(1.0..=8.0).contains(&frame.zoom)
                        || !frame.aspect_ratio.is_finite()
                        || !(0.1..=10.0).contains(&frame.aspect_ratio)
                        || !normalized(frame.position_x)
                        || !normalized(frame.position_y)
                        || frame.highlight.is_some_and(|[x, y, w, h]| {
                            ![x, y, w, h].into_iter().all(normalized)
                                || w == 0.0
                                || h == 0.0
                                || x + w > 1.0
                                || y + h > 1.0
                        })
                    {
                        return Err(invalid("invalid image framing"));
                    }
                }
                if (media.kind != MediaKind::Image && media.poster.is_none())
                    || (media.kind != MediaKind::Video && media.captions.is_some())
                {
                    return Err(invalid(
                        "GIF/video require a poster; captions are only supported for video",
                    ));
                }
            }
        }
        Ok(manifest)
    }
    /// Includes disabled drafts so explicit debug previews can inspect them.
    pub fn release(&self, version: &str) -> Option<&ReleaseHighlights> {
        self.releases
            .iter()
            .find(|r| normalized(&r.version) == normalized(version))
    }
}
impl ReleaseHighlights {
    pub fn should_auto_open(&self, installed_version: &str, seen_versions: &[String]) -> bool {
        self.enabled
            && !self.slides.is_empty()
            && normalized(&self.version) == normalized(installed_version)
            && !seen_versions
                .iter()
                .any(|v| normalized(v) == normalized(&self.version))
    }
    pub fn mark_seen(&self, seen_versions: &mut Vec<String>) {
        if !seen_versions
            .iter()
            .any(|v| normalized(v) == normalized(&self.version))
        {
            seen_versions.push(normalized(&self.version).to_owned());
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn value() -> serde_json::Value {
        serde_json::json!({"schema_version":1,"releases":[{"version":"1.0.2","enabled":true,"title":"What's new","slides":[{"id":"sidebar","title":"More room","description":"Collapse the sidebar.","media":{"kind":"image","src":"highlights/1.0.2/sidebar.png","alt":"Collapsed sidebar"}}]}]})
    }
    fn parse(v: &serde_json::Value) -> Result<Manifest, ManifestError> {
        Manifest::parse(&serde_json::to_vec(v).unwrap())
    }
    #[test]
    fn installed_version_and_seen_versions_are_independent() {
        let manifest = parse(&value()).unwrap();
        let release = manifest.release("v1.0.2").unwrap();
        let mut seen = vec!["1.0.1".into()];
        assert!(release.should_auto_open("1.0.2", &seen));
        assert!(!release.should_auto_open("1.0.3", &seen));
        release.mark_seen(&mut seen);
        release.mark_seen(&mut seen);
        assert_eq!(seen, ["1.0.1", "1.0.2"]);
        assert!(!release.should_auto_open("v1.0.2", &seen));
    }
    #[test]
    fn disabled_draft_is_previewable_but_never_automatic() {
        let mut v = value();
        v["releases"][0]["enabled"] = false.into();
        let m = parse(&v).unwrap();
        assert!(!m.release("1.0.2").unwrap().should_auto_open("1.0.2", &[]));
    }
    #[test]
    fn validates_optional_image_framing() {
        let mut v = value();
        let frame = serde_json::json!({"zoom":2.8,"aspect_ratio":1.597,"position_x":0.0,"position_y":0.0,"highlight":[0.009,0.014,0.022,0.035]});
        v["releases"][0]["slides"][0]["media"]["frame"] = frame.clone();
        assert!(parse(&v).is_ok());
        for (field, bad) in [
            ("zoom", 0.5),
            ("zoom", 9.0),
            ("aspect_ratio", 0.0),
            ("position_x", -0.1),
            ("position_y", 1.1),
        ] {
            let mut invalid = v.clone();
            invalid["releases"][0]["slides"][0]["media"]["frame"][field] = bad.into();
            assert!(parse(&invalid).is_err(), "accepted {field}={bad}");
        }
        v["releases"][0]["slides"][0]["media"]["frame"]["highlight"] =
            serde_json::json!([0.9, 0.0, 0.2, 0.1]);
        assert!(parse(&v).is_err());
        v["releases"][0]["slides"][0]["media"]["frame"] = frame;
        v["releases"][0]["slides"][0]["media"]["kind"] = "gif".into();
        v["releases"][0]["slides"][0]["media"]["poster"] = "highlights/poster.png".into();
        assert!(parse(&v).is_ok());
        v["releases"][0]["slides"][0]["media"]["kind"] = "video".into();
        v["releases"][0]["slides"][0]["media"]["poster"] = "highlights/poster.png".into();
        assert!(parse(&v).is_err());
    }
    #[test]
    fn rejects_unsafe_paths() {
        for path in [
            "/absolute.png",
            "../escape.png",
            "a/../b.png",
            "https://x/image.png",
            "C:\\image.png",
            "a//b",
            "a/%2e%2e/b",
            "",
            "a/./b",
            "fonts/sidebar.png",
            "sidebar.png",
        ] {
            let mut v = value();
            v["releases"][0]["slides"][0]["media"]["src"] = path.into();
            assert!(parse(&v).is_err(), "accepted {path}");
        }
    }
    #[test]
    fn rejects_schema_duplicates_and_missing_content() {
        let mut v = value();
        v["schema_version"] = 2.into();
        assert!(parse(&v).is_err());
        let mut v = value();
        let duplicate = v["releases"][0].clone();
        v["releases"].as_array_mut().unwrap().push(duplicate);
        assert!(parse(&v).is_err());
        let mut v = value();
        let duplicate = v["releases"][0]["slides"][0].clone();
        v["releases"][0]["slides"]
            .as_array_mut()
            .unwrap()
            .push(duplicate);
        assert!(parse(&v).is_err());
        let mut v = value();
        v["releases"][0]["slides"] = serde_json::json!([]);
        assert!(parse(&v).is_err());
        v["releases"][0]["enabled"] = false.into();
        assert!(parse(&v).is_ok());
        let mut v = value();
        v["releases"][0]["slides"][0]["media"]["alt"] = " ".into();
        assert!(parse(&v).is_err());
    }
    #[test]
    fn rejects_animated_media_without_posters_and_nonvideo_captions() {
        for kind in ["gif", "video"] {
            let mut v = value();
            v["releases"][0]["slides"][0]["media"]["kind"] = kind.into();
            assert!(parse(&v).is_err());
            v["releases"][0]["slides"][0]["media"]["poster"] = " ".into();
            assert!(parse(&v).is_err());
        }
        for kind in ["image", "gif"] {
            let mut v = value();
            let media = &mut v["releases"][0]["slides"][0]["media"];
            media["kind"] = kind.into();
            media["poster"] = "highlights/poster.png".into();
            media["captions"] = "highlights/captions.vtt".into();
            assert!(parse(&v).is_err());
        }
    }
    #[test]
    fn accepts_video_and_gif_with_local_posters() {
        for kind in ["video", "gif"] {
            let mut v = value();
            let media = &mut v["releases"][0]["slides"][0]["media"];
            media["kind"] = kind.into();
            assert!(
                parse(&v).is_err(),
                "animated media must have a paused fallback"
            );
            let media = &mut v["releases"][0]["slides"][0]["media"];
            media["poster"] = "highlights/poster.png".into();
            if kind == "video" {
                media["captions"] = "highlights/captions.vtt".into();
            }
            assert!(parse(&v).is_ok());
        }
    }
}
