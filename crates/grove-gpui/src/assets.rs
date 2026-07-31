//! `AssetSource` over the repo's existing `assets/` tree.
//!
//! Only the bundled fonts are consumed in this phase. The SVG icon set
//! (`src/gui/icons.rs`'s generated single-color SVGs) is not wired here —
//! Plan 06 adds it.

use std::borrow::Cow;

use gpui::{AssetSource, SharedString};
use rust_embed::RustEmbed;

/// The repo's `assets/` directory, embedded at compile time.
#[derive(RustEmbed)]
// Relative to this crate's manifest dir, i.e. the repo-root `assets/` the iced
// app already ships. `$CARGO_MANIFEST_DIR` interpolation would need rust-embed's
// `interpolate-folder-path` feature; a plain relative path needs nothing.
#[folder = "../../assets"]
#[include = "fonts/*"]
pub struct Assets;

impl AssetSource for Assets {
    /// A miss is `Ok(None)`, not an `Err` — gpui treats `Err` as a real
    /// failure (bad archive, unreadable bytes), not "not bundled".
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        Ok(Self::get(path).map(|f| f.data))
    }

    fn list(&self, path: &str) -> anyhow::Result<Vec<SharedString>> {
        Ok(Self::iter()
            .filter(|p| p.starts_with(path))
            .map(|p| SharedString::from(p.to_string()))
            .collect())
    }
}
