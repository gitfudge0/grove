//! Curated application theme identities. Terminal-only/custom palettes remain separate.
use crate::theme::{self, Theme, ThemeKind};
use std::borrow::Cow;

#[derive(Clone, Copy, Debug)]
pub struct AppTheme {
    pub id: &'static str,
    pub label: &'static str,
    pub kind: ThemeKind,
    palette: &'static str,
}

macro_rules! entry {
    ($id:literal, $label:literal, $kind:ident, $palette:literal) => {
        AppTheme {
            id: $id,
            label: $label,
            kind: ThemeKind::$kind,
            palette: $palette,
        }
    };
}
pub const CATALOG: &[AppTheme] = &[
    entry!("grove-light", "Grove Light", Light, "tokyonight-day"),
    entry!("grove-dark", "Grove Dark", Dark, "tokyonight-storm"),
    entry!("nord", "Nord", Dark, "nord"),
    entry!(
        "catppuccin-latte",
        "Catppuccin Latte",
        Light,
        "catppuccin-latte"
    ),
    entry!(
        "catppuccin-frappe",
        "Catppuccin Frappé",
        Dark,
        "catppuccin-frappe"
    ),
    entry!(
        "catppuccin-macchiato",
        "Catppuccin Macchiato",
        Dark,
        "catppuccin-macchiato"
    ),
    entry!("catppuccin-mocha", "Catppuccin Mocha", Dark, "catppuccin"),
    entry!("tokyo-night", "Tokyo Night", Dark, "tokyonight"),
    entry!(
        "tokyo-night-storm",
        "Tokyo Night Storm",
        Dark,
        "tokyonight-storm"
    ),
    entry!(
        "tokyo-night-day",
        "Tokyo Night Day",
        Light,
        "tokyonight-day"
    ),
    entry!("dracula", "Dracula", Dark, "dracula"),
    entry!("one-dark", "One Dark", Dark, "one-dark"),
    entry!("github-light", "GitHub Light", Light, "github-light"),
    entry!("github-dark", "GitHub Dark", Dark, "github-dark"),
    entry!("gruvbox-light", "Gruvbox Light", Light, "gruvbox-light"),
    entry!("gruvbox-dark", "Gruvbox Dark", Dark, "gruvbox"),
    entry!(
        "solarized-light",
        "Solarized Light",
        Light,
        "solarized-light"
    ),
    entry!("solarized-dark", "Solarized Dark", Dark, "solarized-dark"),
    entry!("rose-pine", "Rosé Pine", Dark, "rose-pine"),
    entry!("rose-pine-moon", "Rosé Pine Moon", Dark, "rose-pine-moon"),
    entry!("rose-pine-dawn", "Rosé Pine Dawn", Light, "rose-pine-dawn"),
];

pub fn by_id(id: &str) -> Option<&'static AppTheme> {
    CATALOG.iter().find(|item| item.id == id)
}

pub fn search_key(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .map(|c| match c {
            'é' | 'è' | 'ê' | 'ë' => 'e',
            c => c,
        })
        .collect()
}

fn color(hex: u32) -> theme::Color {
    theme::Color::Rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

impl AppTheme {
    pub fn matches(&self, query: &str, kind: Option<ThemeKind>) -> bool {
        kind.is_none_or(|kind| kind == self.kind)
            && search_key(self.label).contains(&search_key(query.trim()))
    }

    pub fn terminal(&self) -> Theme {
        let mut palette = match self.palette {
            "nord" => from_colors([
                0x2e3440, 0x434c5e, 0xd8dee9, 0xbfc7d5, 0x7b88a1, 0x81a1c1, 0x88c0d0, 0xb48ead,
                0xa3be8c, 0xebcb8b, 0xbf616a,
            ]),
            "catppuccin-frappe" => from_colors([
                0x303446, 0x414559, 0xc6d0f5, 0xa5adce, 0x737994, 0x8caaee, 0x81c8be, 0xca9ee6,
                0xa6d189, 0xe5c890, 0xe78284,
            ]),
            "catppuccin-macchiato" => from_colors([
                0x24273a, 0x363a4f, 0xcad3f5, 0xa5adcb, 0x6e738d, 0x8aadf4, 0x8bd5ca, 0xc6a0f6,
                0xa6da95, 0xeed49f, 0xed8796,
            ]),
            "solarized-light" => from_colors([
                0xfdf6e3, 0xeee8d5, 0x586e75, 0x657b83, 0x839496, 0x268bd2, 0x2aa198, 0x6c71c4,
                0x859900, 0xb58900, 0xdc322f,
            ]),
            "solarized-dark" => from_colors([
                0x002b36, 0x073642, 0x93a1a1, 0x839496, 0x657b83, 0x268bd2, 0x2aa198, 0x6c71c4,
                0x859900, 0xb58900, 0xdc322f,
            ]),
            "rose-pine-moon" => from_colors([
                0x232136, 0x393552, 0xe0def4, 0x908caa, 0x6e6a86, 0x9ccfd8, 0x9ccfd8, 0xc4a7e7,
                0x3e8fb0, 0xf6c177, 0xeb6f92,
            ]),
            name => theme::BUILTINS
                .iter()
                .find(|theme| theme.name == name)
                .cloned()
                .unwrap_or(theme::TOKYONIGHT),
        };
        palette.name = Cow::Borrowed(self.id);
        palette.kind = self.kind;
        palette
    }
}

fn from_colors(colors: [u32; 11]) -> Theme {
    let mut result = theme::TOKYONIGHT.clone();
    for (index, value) in colors.into_iter().enumerate() {
        result.set_field(index, color(value));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_catalog_identities_resolve_and_search_without_accents() {
        let mut ids = std::collections::HashSet::new();
        assert_eq!(CATALOG.len(), 21);
        for item in CATALOG {
            assert!(ids.insert(item.id));
            let palette = item.terminal();
            assert_eq!(palette.name, item.id);
            assert_eq!(palette.kind, item.kind);
            assert!(item.matches(item.label, Some(item.kind)));
            assert!(
                theme::contrast_ratio(palette.fg, palette.bg) >= 4.5,
                "{}",
                item.label
            );
        }
        assert!(by_id("rose-pine").unwrap().matches("rose pine", None));
        assert!(by_id("catppuccin-frappe").unwrap().matches("frappe", None));
    }
}
