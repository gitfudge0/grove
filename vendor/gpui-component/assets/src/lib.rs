/// Embeds icon SVGs for [IconName]. Native embeds via RustEmbed; WASM downloads from CDN on demand and caches in memory.
///
/// ```rust,no_run
/// use gpui::*;
/// use gpui_component_assets::Assets;
///
/// let app = gpui_platform::application().with_assets(Assets);
/// ```
#[cfg(not(target_family = "wasm"))]
mod native_assets;

#[cfg(target_family = "wasm")]
mod wasm_assets;

#[cfg(not(target_family = "wasm"))]
pub use native_assets::Assets;

#[cfg(target_family = "wasm")]
pub use wasm_assets::Assets;
