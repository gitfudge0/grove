//! Two sources behind one `AssetSource`: bundled fonts embedded from `assets/`, and the SVG icon set generated in memory by [`crate::icons`] instead of shipped as files.

use std::borrow::Cow;

use gpui::{AssetSource, SharedString};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
// Relative to this crate's manifest dir, which is the repo root.
// `$CARGO_MANIFEST_DIR` interpolation would need rust-embed's `interpolate-folder-path` feature; a plain relative path needs nothing.
#[folder = "assets"]
#[include = "fonts/*"]
pub struct Assets;

impl AssetSource for Assets {
    /// A miss is `Ok(None)`, not an `Err` — gpui treats `Err` as a real failure (bad archive, unreadable bytes), not "not bundled".
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        if let Some(svg) = crate::icons::svg_for_path(path) {
            return Ok(Some(Cow::Owned(svg.into_bytes())));
        }
        Ok(Self::get(path).map(|f| f.data))
    }

    /// The generated icons have no directory to enumerate, and nothing in gpui discovers assets by listing.
    fn list(&self, path: &str) -> anyhow::Result<Vec<SharedString>> {
        Ok(Self::iter()
            .filter(|p| p.starts_with(path))
            .map(|p| SharedString::from(p.to_string()))
            .collect())
    }
}
