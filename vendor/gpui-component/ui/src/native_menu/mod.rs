//! A menu rendered natively by the OS, unlike [`crate::menu::PopupMenu`] which GPUI draws clipped to the window bounds.
//! Items carry a GPUI [`Action`] dispatched via [`Window::dispatch_action`], so a [`NativeMenu`] can be built directly from [`gpui::MenuItem`]s (see [`From<gpui::Menu>`]).
//!
//! ```ignore
//! use gpui_component::native_menu::NativeMenu;
//!
//! NativeMenu::new()
//!     .menu("Copy", Box::new(Copy))
//!     .menu("Paste", Box::new(Paste))
//!     .separator()
//!     .menu("Delete", Box::new(Delete))
//!     .show(position, window, cx);
//! ```

use crate::Icon;
#[cfg(target_os = "windows")]
use crate::ActiveTheme as _;

#[cfg(any(target_os = "macos", target_os = "windows"))]
use gpui::AssetSource;
use gpui::{Action, App, Pixels, Point, SharedString, Window};
#[cfg(any(target_os = "macos", target_os = "windows"))]
use gpui::{Image, ImageFormat};
#[cfg(any(target_os = "macos", target_os = "windows"))]
use std::{path::Path, sync::Arc};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

// Drawn-menu fallback for platforms without an OS-native popup (e.g. Linux); compiled everywhere because `Root` holds the overlay entity.
mod fallback;
pub(crate) use fallback::FallbackMenuOverlay;

enum NativeMenuItem {
    Separator,
    Item {
        label: SharedString,
        disabled: bool,
        checked: bool,
        icon: Option<Box<Icon>>,
        action: Option<Box<dyn Action>>,
    },
    Submenu {
        label: SharedString,
        disabled: bool,
        items: Vec<NativeMenuItem>,
    },
}

#[derive(Default)]
pub struct NativeMenu {
    items: Vec<NativeMenuItem>,
}

impl NativeMenu {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn menu(self, label: impl Into<SharedString>, action: Box<dyn Action>) -> Self {
        self.menu_with(label, false, false, None, Some(action))
    }

    pub fn menu_with_disabled(
        self,
        label: impl Into<SharedString>,
        disabled: bool,
        action: Box<dyn Action>,
    ) -> Self {
        self.menu_with(label, disabled, false, None, Some(action))
    }

    pub fn menu_with_check(
        self,
        label: impl Into<SharedString>,
        checked: bool,
        action: Box<dyn Action>,
    ) -> Self {
        self.menu_with(label, false, checked, None, Some(action))
    }

    /// This is the item's content icon, not its check-mark indicator. macOS loads it as an `NSImage` template (tints with item text); Windows as an `HBITMAP`, SVG rasterized via `resvg`.
    pub fn menu_with_icon(
        self,
        label: impl Into<SharedString>,
        icon: impl Into<Icon>,
        action: Box<dyn Action>,
    ) -> Self {
        self.menu_with(label, false, false, Some(icon.into()), Some(action))
    }

    pub fn menu_with_icon_disabled(
        self,
        label: impl Into<SharedString>,
        icon: impl Into<Icon>,
        disabled: bool,
        action: Box<dyn Action>,
    ) -> Self {
        self.menu_with(label, disabled, false, Some(icon.into()), Some(action))
    }

    /// Alias for [`Self::menu_with_icon_disabled`], matching [`crate::menu::PopupMenu`].
    pub fn menu_with_icon_and_disabled(
        self,
        label: impl Into<SharedString>,
        icon: impl Into<Icon>,
        action: Box<dyn Action>,
        disabled: bool,
    ) -> Self {
        self.menu_with_icon_disabled(label, icon, disabled, action)
    }

    fn menu_with(
        mut self,
        label: impl Into<SharedString>,
        disabled: bool,
        checked: bool,
        icon: Option<Icon>,
        action: Option<Box<dyn Action>>,
    ) -> Self {
        self.items.push(NativeMenuItem::Item {
            label: label.into(),
            disabled,
            checked,
            icon: icon.map(Box::new),
            action,
        });
        self
    }

    pub fn separator(mut self) -> Self {
        self.items.push(NativeMenuItem::Separator);
        self
    }

    pub fn submenu(mut self, label: impl Into<SharedString>, submenu: NativeMenu) -> Self {
        self.items.push(NativeMenuItem::Submenu {
            label: label.into(),
            disabled: false,
            items: submenu.items,
        });
        self
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The OS tracking loop runs off GPUI's call stack, so GPUI is never borrowed while the menu is open.
    pub fn show(self, position: Point<Pixels>, window: &mut Window, cx: &mut App) {
        if self.items.is_empty() {
            return;
        }

        #[cfg(target_os = "macos")]
        {
            macos::show(self.items, cx.asset_source().clone(), position, window, cx);
        }
        #[cfg(target_os = "windows")]
        {
            windows::show(
                self.items,
                cx.asset_source().clone(),
                position,
                cx.theme().is_dark(),
                window,
                cx,
            );
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        fallback::show(self.items, position, window, cx);
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub(super) fn resolve_icon_image(
    path: &SharedString,
    asset_source: &dyn AssetSource,
) -> Option<Arc<Image>> {
    if path.is_empty() {
        return None;
    }

    let bytes = if Path::new(path.as_ref()).is_file() {
        std::fs::read(path.as_ref()).ok()?
    } else {
        asset_source
            .load(path.as_ref())
            .ok()
            .flatten()?
            .into_owned()
    };
    let format = image_format(path.as_ref(), &bytes)?;
    Some(Arc::new(Image::from_bytes(format, bytes)))
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn image_format(path: &str, bytes: &[u8]) -> Option<ImageFormat> {
    if let Some(extension) = Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
    {
        let format = match extension.to_ascii_lowercase().as_str() {
            "png" => ImageFormat::Png,
            "jpg" | "jpeg" => ImageFormat::Jpeg,
            "webp" => ImageFormat::Webp,
            "gif" => ImageFormat::Gif,
            "svg" => ImageFormat::Svg,
            "bmp" => ImageFormat::Bmp,
            "tif" | "tiff" => ImageFormat::Tiff,
            "ico" => ImageFormat::Ico,
            "pbm" | "pgm" | "ppm" | "pnm" => ImageFormat::Pnm,
            _ => return None,
        };
        return Some(format);
    }

    image_format_from_bytes(bytes)
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn image_format_from_bytes(bytes: &[u8]) -> Option<ImageFormat> {
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(ImageFormat::Png)
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        Some(ImageFormat::Jpeg)
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some(ImageFormat::Gif)
    } else if bytes.starts_with(b"BM") {
        Some(ImageFormat::Bmp)
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        Some(ImageFormat::Webp)
    } else if bytes.starts_with(b"II*\0") || bytes.starts_with(b"MM\0*") {
        Some(ImageFormat::Tiff)
    } else if bytes.starts_with(b"\0\0\x01\0") || bytes.starts_with(b"\0\0\x02\0") {
        Some(ImageFormat::Ico)
    } else if is_svg_bytes(bytes) {
        Some(ImageFormat::Svg)
    } else if matches!(
        bytes.get(0..2),
        Some(b"P1" | b"P2" | b"P3" | b"P4" | b"P5" | b"P6")
    ) {
        Some(ImageFormat::Pnm)
    } else {
        None
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn is_svg_bytes(bytes: &[u8]) -> bool {
    let text = match std::str::from_utf8(&bytes[..bytes.len().min(256)]) {
        Ok(text) => text.trim_start(),
        Err(_) => return false,
    };
    text.starts_with("<svg") || text.starts_with("<?xml")
}

/// System menus (e.g. macOS Services) have no native popup equivalent and are skipped.
impl From<gpui::Menu> for NativeMenu {
    fn from(menu: gpui::Menu) -> Self {
        let mut native = Self::new();
        for item in menu.items {
            match item {
                gpui::MenuItem::Separator => native.items.push(NativeMenuItem::Separator),
                gpui::MenuItem::Action {
                    name,
                    action,
                    checked,
                    disabled,
                    ..
                } => native.items.push(NativeMenuItem::Item {
                    label: name,
                    disabled,
                    checked,
                    icon: None,
                    action: Some(action),
                }),
                gpui::MenuItem::Submenu(submenu) => native.items.push(NativeMenuItem::Submenu {
                    label: submenu.name.clone(),
                    disabled: submenu.disabled,
                    items: Self::from(submenu).items,
                }),
                gpui::MenuItem::SystemMenu(_) => {}
            }
        }
        native
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::IconName;
    use serde::Deserialize;

    #[derive(Action, Clone, PartialEq, Deserialize)]
    #[action(namespace = native_menu_tests, no_json)]
    struct TestAction;

    #[test]
    fn test_native_menu_builder_accepts_icon() {
        let menu =
            NativeMenu::new().menu_with_icon("Github", IconName::Github, Box::new(TestAction));

        assert_eq!(menu.items.len(), 1);
        let NativeMenuItem::Item {
            label,
            disabled,
            checked,
            icon: Some(icon),
            action: Some(_),
        } = &menu.items[0]
        else {
            panic!("expected an actionable item with an icon");
        };

        assert_eq!(label, "Github");
        assert!(!disabled);
        assert!(!checked);
        assert!(icon.path_ref().ends_with("github.svg"));
    }

    #[test]
    fn test_native_menu_builder_accepts_icon_and_disabled_alias() {
        let menu = NativeMenu::new().menu_with_icon_and_disabled(
            "Inbox",
            IconName::Inbox,
            Box::new(TestAction),
            true,
        );

        assert_eq!(menu.items.len(), 1);
        let NativeMenuItem::Item {
            label,
            disabled,
            checked,
            icon: Some(icon),
            action: Some(_),
        } = &menu.items[0]
        else {
            panic!("expected a disabled actionable item with an icon");
        };

        assert_eq!(label, "Inbox");
        assert!(disabled);
        assert!(!checked);
        assert!(icon.path_ref().ends_with("inbox.svg"));
    }

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    #[test]
    fn test_native_menu_icon_asset_resolves_to_bytes() {
        let icon = Icon::new(IconName::Github);
        let image = resolve_icon_image(icon.path_ref(), &gpui_component_assets::Assets)
            .expect("icon asset should resolve");

        assert_eq!(image.format, ImageFormat::Svg);
        assert!(!image.bytes.is_empty());
    }
}
