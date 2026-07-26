use std::borrow::Cow;
use std::cell::RefCell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, RwLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeKind {
    Dark,
    Light,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Color {
    Rgb(u8, u8, u8),
}

#[derive(Clone)]
pub struct Theme {
    pub name: Cow<'static, str>,
    pub kind: ThemeKind,
    pub bg: Color,
    pub bg_highlight: Color,
    pub fg: Color,
    pub fg_dark: Color,
    pub comment: Color,
    pub blue: Color,
    pub cyan: Color,
    pub magenta: Color,
    pub green: Color,
    pub yellow: Color,
    pub red: Color,
}

const fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::Rgb(r, g, b)
}

/// The 11 editable color fields, in the theme editor's fixed row order
/// (`Theme::field`/`set_field` index into this same order). Grouped
/// Surfaces / Text / Accents, matching the editor's section headers.
pub const FIELD_NAMES: [&str; 11] = [
    "bg",
    "bg_highlight",
    "fg",
    "fg_dark",
    "comment",
    "blue",
    "cyan",
    "magenta",
    "green",
    "yellow",
    "red",
];

/// Section label for each index into [`FIELD_NAMES`].
pub const FIELD_GROUPS: [&str; 11] = [
    "Surfaces", "Surfaces", "Text", "Text", "Text", "Accents", "Accents", "Accents", "Accents",
    "Accents", "Accents",
];

/// For each editable field, the index of the field it's contrast-checked
/// against in the theme editor (`None` for `bg`, which has no pair of its
/// own) — `bg_highlight` is checked against `fg` (the text that sits on
/// it), everything else against `bg`.
pub const CONTRAST_PARTNER: [Option<usize>; 11] = [
    None,
    Some(2),
    Some(0),
    Some(0),
    Some(0),
    Some(0),
    Some(0),
    Some(0),
    Some(0),
    Some(0),
    Some(0),
];

impl Theme {
    /// Reads one of the 11 editable colors by index (`FIELD_NAMES` order).
    /// Out-of-range indices fall back to `bg` rather than panicking, since
    /// this is only ever driven by a `0..11` UI row cursor.
    pub fn field(&self, i: usize) -> Color {
        match i {
            0 => self.bg,
            1 => self.bg_highlight,
            2 => self.fg,
            3 => self.fg_dark,
            4 => self.comment,
            5 => self.blue,
            6 => self.cyan,
            7 => self.magenta,
            8 => self.green,
            9 => self.yellow,
            10 => self.red,
            _ => self.bg,
        }
    }

    /// Writes one of the 11 editable colors by index. Out-of-range indices
    /// are a no-op (same defensive contract as `field`).
    pub fn set_field(&mut self, i: usize, c: Color) {
        match i {
            0 => self.bg = c,
            1 => self.bg_highlight = c,
            2 => self.fg = c,
            3 => self.fg_dark = c,
            4 => self.comment = c,
            5 => self.blue = c,
            6 => self.cyan = c,
            7 => self.magenta = c,
            8 => self.green = c,
            9 => self.yellow = c,
            10 => self.red = c,
            _ => {}
        }
    }

    /// Whether every one of the 11 editable colors matches `other` — the
    /// theme editor's dirty check (name/kind aren't editable there, so they
    /// don't factor in).
    pub fn colors_eq(&self, other: &Theme) -> bool {
        (0..FIELD_NAMES.len()).all(|i| self.field(i) == other.field(i))
    }
}

/// WCAG 2.x relative luminance of an sRGB color (the sRGB → linear gamma
/// correction, then the standard 0.2126/0.7152/0.0722 luma weights).
pub fn relative_luminance(c: Color) -> f64 {
    let Color::Rgb(r, g, b) = c;
    let chan = |v: u8| -> f64 {
        let s = v as f64 / 255.0;
        if s <= 0.039_28 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * chan(r) + 0.7152 * chan(g) + 0.0722 * chan(b)
}

/// WCAG 2.x contrast ratio between two colors: `(L1 + 0.05) / (L2 + 0.05)`
/// with `L1` the lighter of the two relative luminances, always in `[1.0,
/// 21.0]`. Used for the theme editor's amber/red contrast badges — warns
/// only, never blocks saving.
pub fn contrast_ratio(a: Color, b: Color) -> f64 {
    let la = relative_luminance(a) + 0.05;
    let lb = relative_luminance(b) + 0.05;
    if la > lb {
        la / lb
    } else {
        lb / la
    }
}

// ---------------- Dark themes ----------------

pub const TOKYONIGHT: Theme = Theme {
    name: Cow::Borrowed("tokyonight"),
    kind: ThemeKind::Dark,
    bg: rgb(0x1a, 0x1b, 0x26),
    bg_highlight: rgb(0x29, 0x2e, 0x42),
    fg: rgb(0xc0, 0xca, 0xf5),
    fg_dark: rgb(0xa9, 0xb1, 0xd6),
    comment: rgb(0x56, 0x5f, 0x89),
    blue: rgb(0x7a, 0xa2, 0xf7),
    cyan: rgb(0x7d, 0xcf, 0xff),
    magenta: rgb(0xbb, 0x9a, 0xf7),
    green: rgb(0x9e, 0xce, 0x6a),
    yellow: rgb(0xe0, 0xaf, 0x68),
    red: rgb(0xf7, 0x76, 0x8e),
};

pub const EVERFOREST: Theme = Theme {
    name: Cow::Borrowed("everforest"),
    kind: ThemeKind::Dark,
    bg: rgb(0x2d, 0x35, 0x3b),
    bg_highlight: rgb(0x3d, 0x48, 0x4d),
    fg: rgb(0xd3, 0xc6, 0xaa),
    fg_dark: rgb(0x9d, 0xa9, 0xa0),
    comment: rgb(0x7a, 0x84, 0x78),
    blue: rgb(0x7f, 0xbb, 0xb3),
    cyan: rgb(0x83, 0xc0, 0x92),
    magenta: rgb(0xd6, 0x99, 0xb6),
    green: rgb(0xa7, 0xc0, 0x80),
    yellow: rgb(0xdb, 0xbc, 0x7f),
    red: rgb(0xe6, 0x7e, 0x80),
};

pub const CATPPUCCIN: Theme = Theme {
    name: Cow::Borrowed("catppuccin"),
    kind: ThemeKind::Dark,
    bg: rgb(0x1e, 0x1e, 0x2e),
    bg_highlight: rgb(0x31, 0x32, 0x44),
    fg: rgb(0xcd, 0xd6, 0xf4),
    fg_dark: rgb(0xa6, 0xad, 0xc8),
    comment: rgb(0x6c, 0x70, 0x86),
    blue: rgb(0x89, 0xb4, 0xfa),
    cyan: rgb(0x94, 0xe2, 0xd5),
    magenta: rgb(0xcb, 0xa6, 0xf7),
    green: rgb(0xa6, 0xe3, 0xa1),
    yellow: rgb(0xf9, 0xe2, 0xaf),
    red: rgb(0xf3, 0x8b, 0xa8),
};

pub const GRUVBOX: Theme = Theme {
    name: Cow::Borrowed("gruvbox"),
    kind: ThemeKind::Dark,
    bg: rgb(0x28, 0x28, 0x28),
    bg_highlight: rgb(0x3c, 0x38, 0x36),
    fg: rgb(0xeb, 0xdb, 0xb2),
    fg_dark: rgb(0xa8, 0x99, 0x84),
    comment: rgb(0x92, 0x83, 0x74),
    blue: rgb(0x83, 0xa5, 0x98),
    cyan: rgb(0x8e, 0xc0, 0x7c),
    magenta: rgb(0xd3, 0x86, 0x9b),
    green: rgb(0xb8, 0xbb, 0x26),
    yellow: rgb(0xfa, 0xbd, 0x2f),
    red: rgb(0xfb, 0x49, 0x34),
};

pub const KANAGAWA: Theme = Theme {
    name: Cow::Borrowed("kanagawa"),
    kind: ThemeKind::Dark,
    bg: rgb(0x1f, 0x1f, 0x28),
    bg_highlight: rgb(0x2a, 0x2a, 0x37),
    fg: rgb(0xdc, 0xd7, 0xba),
    fg_dark: rgb(0xc8, 0xc0, 0x93),
    comment: rgb(0x72, 0x71, 0x69),
    blue: rgb(0x7e, 0x9c, 0xd8),
    cyan: rgb(0x7f, 0xb4, 0xca),
    magenta: rgb(0x95, 0x7f, 0xb8),
    green: rgb(0x98, 0xbb, 0x6c),
    yellow: rgb(0xe6, 0xc3, 0x84),
    red: rgb(0xe4, 0x68, 0x76),
};

pub const ONE_DARK: Theme = Theme {
    name: Cow::Borrowed("one-dark"),
    kind: ThemeKind::Dark,
    bg: rgb(0x28, 0x2c, 0x34),
    bg_highlight: rgb(0x3e, 0x44, 0x51),
    fg: rgb(0xab, 0xb2, 0xbf),
    fg_dark: rgb(0x82, 0x89, 0x97),
    comment: rgb(0x5c, 0x63, 0x70),
    blue: rgb(0x61, 0xaf, 0xef),
    cyan: rgb(0x56, 0xb6, 0xc2),
    magenta: rgb(0xc6, 0x78, 0xdd),
    green: rgb(0x98, 0xc3, 0x79),
    yellow: rgb(0xe5, 0xc0, 0x7b),
    red: rgb(0xe0, 0x6c, 0x75),
};

pub const DRACULA: Theme = Theme {
    name: Cow::Borrowed("dracula"),
    kind: ThemeKind::Dark,
    bg: rgb(0x28, 0x2a, 0x36),
    bg_highlight: rgb(0x44, 0x47, 0x5a),
    fg: rgb(0xf8, 0xf8, 0xf2),
    fg_dark: rgb(0xbf, 0xbf, 0xbf),
    comment: rgb(0x62, 0x72, 0xa4),
    blue: rgb(0xbd, 0x93, 0xf9),
    cyan: rgb(0x8b, 0xe9, 0xfd),
    magenta: rgb(0xff, 0x79, 0xc6),
    green: rgb(0x50, 0xfa, 0x7b),
    yellow: rgb(0xf1, 0xfa, 0x8c),
    red: rgb(0xff, 0x55, 0x55),
};

pub const ROSE_PINE: Theme = Theme {
    name: Cow::Borrowed("rose-pine"),
    kind: ThemeKind::Dark,
    bg: rgb(0x19, 0x17, 0x24),
    bg_highlight: rgb(0x26, 0x23, 0x3a),
    fg: rgb(0xe0, 0xde, 0xf4),
    fg_dark: rgb(0x90, 0x8c, 0xaa),
    comment: rgb(0x6e, 0x6a, 0x86),
    blue: rgb(0x31, 0x74, 0x8f),
    cyan: rgb(0x9c, 0xcf, 0xd8),
    magenta: rgb(0xc4, 0xa7, 0xe7),
    green: rgb(0x9c, 0xcf, 0xd8),
    yellow: rgb(0xf6, 0xc1, 0x77),
    red: rgb(0xeb, 0x6f, 0x92),
};

pub const GITHUB_DARK: Theme = Theme {
    name: Cow::Borrowed("github-dark"),
    kind: ThemeKind::Dark,
    bg: rgb(0x0d, 0x11, 0x17),
    bg_highlight: rgb(0x16, 0x1b, 0x22),
    fg: rgb(0xe6, 0xed, 0xf3),
    fg_dark: rgb(0x7d, 0x85, 0x90),
    comment: rgb(0x8b, 0x94, 0x9e),
    blue: rgb(0x58, 0xa6, 0xff),
    cyan: rgb(0x39, 0xc5, 0xcf),
    magenta: rgb(0xbc, 0x8c, 0xff),
    green: rgb(0x3f, 0xb9, 0x50),
    yellow: rgb(0xd2, 0x99, 0x22),
    red: rgb(0xf8, 0x51, 0x49),
};

// ---------------- Light themes ----------------

pub const GITHUB_LIGHT: Theme = Theme {
    name: Cow::Borrowed("github-light"),
    kind: ThemeKind::Light,
    bg: rgb(0xff, 0xff, 0xff),
    bg_highlight: rgb(0xf6, 0xf8, 0xfa),
    fg: rgb(0x24, 0x29, 0x2f),
    fg_dark: rgb(0x57, 0x60, 0x6a),
    comment: rgb(0x6e, 0x77, 0x81),
    blue: rgb(0x09, 0x69, 0xda),
    cyan: rgb(0x1f, 0x6f, 0xeb),
    magenta: rgb(0x82, 0x50, 0xdf),
    green: rgb(0x1a, 0x7f, 0x37),
    yellow: rgb(0x9a, 0x67, 0x00),
    red: rgb(0xcf, 0x22, 0x2e),
};

pub const GRUVBOX_LIGHT: Theme = Theme {
    name: Cow::Borrowed("gruvbox-light"),
    kind: ThemeKind::Light,
    bg: rgb(0xfb, 0xf1, 0xc7),
    bg_highlight: rgb(0xeb, 0xdb, 0xb2),
    fg: rgb(0x3c, 0x38, 0x36),
    fg_dark: rgb(0x50, 0x49, 0x45),
    comment: rgb(0x7c, 0x6f, 0x64),
    blue: rgb(0x07, 0x66, 0x78),
    cyan: rgb(0x42, 0x7b, 0x58),
    magenta: rgb(0x8f, 0x3f, 0x71),
    green: rgb(0x79, 0x74, 0x0e),
    yellow: rgb(0xb5, 0x76, 0x14),
    red: rgb(0x9d, 0x00, 0x06),
};

pub const EVERFOREST_LIGHT: Theme = Theme {
    name: Cow::Borrowed("everforest-light"),
    kind: ThemeKind::Light,
    bg: rgb(0xfd, 0xf6, 0xe3),
    bg_highlight: rgb(0xf4, 0xf0, 0xd9),
    fg: rgb(0x5c, 0x6a, 0x72),
    fg_dark: rgb(0x82, 0x91, 0x81),
    comment: rgb(0xa6, 0xb0, 0xa0),
    blue: rgb(0x3a, 0x94, 0xc5),
    cyan: rgb(0x35, 0xa7, 0x7c),
    magenta: rgb(0xdf, 0x69, 0xba),
    green: rgb(0x8d, 0xa1, 0x01),
    yellow: rgb(0xdf, 0xa0, 0x00),
    red: rgb(0xf8, 0x55, 0x52),
};

pub const ONE_LIGHT: Theme = Theme {
    name: Cow::Borrowed("one-light"),
    kind: ThemeKind::Light,
    bg: rgb(0xfa, 0xfa, 0xfa),
    bg_highlight: rgb(0xe5, 0xe5, 0xe6),
    fg: rgb(0x38, 0x3a, 0x42),
    fg_dark: rgb(0x69, 0x6c, 0x77),
    comment: rgb(0xa0, 0xa1, 0xa7),
    blue: rgb(0x40, 0x78, 0xf2),
    cyan: rgb(0x01, 0x84, 0xbc),
    magenta: rgb(0xa6, 0x26, 0xa4),
    green: rgb(0x50, 0xa1, 0x4f),
    yellow: rgb(0xc1, 0x84, 0x01),
    red: rgb(0xe4, 0x56, 0x49),
};

pub const CATPPUCCIN_LATTE: Theme = Theme {
    name: Cow::Borrowed("catppuccin-latte"),
    kind: ThemeKind::Light,
    bg: rgb(0xef, 0xf1, 0xf5),
    bg_highlight: rgb(0xcc, 0xd0, 0xda),
    fg: rgb(0x4c, 0x4f, 0x69),
    fg_dark: rgb(0x5c, 0x5f, 0x77),
    comment: rgb(0x6c, 0x6f, 0x85),
    blue: rgb(0x1e, 0x66, 0xf5),
    cyan: rgb(0x17, 0x92, 0x99),
    magenta: rgb(0x88, 0x39, 0xef),
    green: rgb(0x40, 0xa0, 0x2b),
    yellow: rgb(0xdf, 0x8e, 0x1d),
    red: rgb(0xd2, 0x0f, 0x39),
};

pub const ROSE_PINE_DAWN: Theme = Theme {
    name: Cow::Borrowed("rose-pine-dawn"),
    kind: ThemeKind::Light,
    bg: rgb(0xfa, 0xf4, 0xed),
    bg_highlight: rgb(0xf2, 0xe9, 0xe1),
    fg: rgb(0x57, 0x52, 0x79),
    fg_dark: rgb(0x79, 0x75, 0x93),
    comment: rgb(0x98, 0x93, 0xa5),
    blue: rgb(0x28, 0x69, 0x83),
    cyan: rgb(0x56, 0x94, 0x9f),
    magenta: rgb(0x90, 0x7a, 0xa9),
    green: rgb(0x56, 0x94, 0x9f),
    yellow: rgb(0xea, 0x9d, 0x34),
    red: rgb(0xb4, 0x63, 0x7a),
};

pub const TOKYONIGHT_DAY: Theme = Theme {
    name: Cow::Borrowed("tokyonight-day"),
    kind: ThemeKind::Light,
    bg: rgb(0xe1, 0xe2, 0xe7),
    bg_highlight: rgb(0xc4, 0xc8, 0xda),
    fg: rgb(0x37, 0x60, 0xbf),
    fg_dark: rgb(0x61, 0x72, 0xb0),
    comment: rgb(0x84, 0x8c, 0xb5),
    blue: rgb(0x2e, 0x7d, 0xe9),
    cyan: rgb(0x00, 0x71, 0x97),
    magenta: rgb(0x98, 0x54, 0xf1),
    green: rgb(0x58, 0x75, 0x39),
    yellow: rgb(0x8c, 0x6c, 0x3e),
    red: rgb(0xf5, 0x2a, 0x65),
};

pub const VSCODE_DARK_MODERN: Theme = Theme {
    name: Cow::Borrowed("vscode-dark-modern"),
    kind: ThemeKind::Dark,
    bg: rgb(0x1f, 0x1f, 0x1f),
    bg_highlight: rgb(0x2a, 0x2a, 0x2a),
    fg: rgb(0xcc, 0xcc, 0xcc),
    fg_dark: rgb(0x9d, 0x9d, 0x9d),
    comment: rgb(0x6a, 0x99, 0x55),
    blue: rgb(0x56, 0x9c, 0xd6),
    cyan: rgb(0x4e, 0xc9, 0xb0),
    magenta: rgb(0xc5, 0x86, 0xc0),
    green: rgb(0x6a, 0x99, 0x55),
    yellow: rgb(0xdc, 0xdc, 0xaa),
    red: rgb(0xf4, 0x47, 0x47),
};

pub const TOKYONIGHT_STORM: Theme = Theme {
    name: Cow::Borrowed("tokyonight-storm"),
    kind: ThemeKind::Dark,
    bg: rgb(0x24, 0x28, 0x3b),
    bg_highlight: rgb(0x37, 0x3b, 0x51),
    fg: rgb(0xc0, 0xca, 0xf5),
    fg_dark: rgb(0xa1, 0xaa, 0xd0),
    comment: rgb(0x56, 0x5f, 0x89),
    blue: rgb(0x7a, 0xa2, 0xf7),
    cyan: rgb(0x7d, 0xcf, 0xff),
    magenta: rgb(0xbb, 0x9a, 0xf7),
    green: rgb(0x9e, 0xce, 0x6a),
    yellow: rgb(0xe0, 0xaf, 0x68),
    red: rgb(0xf7, 0x76, 0x8e),
};

pub const VITESSE_DARK: Theme = Theme {
    name: Cow::Borrowed("vitesse-dark"),
    kind: ThemeKind::Dark,
    bg: rgb(0x12, 0x12, 0x12),
    bg_highlight: rgb(0x2a, 0x2a, 0x28),
    fg: rgb(0xdb, 0xd7, 0xca),
    fg_dark: rgb(0xb3, 0xb0, 0xa5),
    comment: rgb(0x75, 0x85, 0x75),
    blue: rgb(0x63, 0x94, 0xbf),
    cyan: rgb(0x5e, 0xaa, 0xb5),
    magenta: rgb(0xd9, 0x73, 0x9f),
    green: rgb(0x4d, 0x93, 0x75),
    yellow: rgb(0xe6, 0xcc, 0x77),
    red: rgb(0xcb, 0x76, 0x76),
};

pub const GITHUB_DARK_DIMMED: Theme = Theme {
    name: Cow::Borrowed("github-dark-dimmed"),
    kind: ThemeKind::Dark,
    bg: rgb(0x22, 0x27, 0x2e),
    bg_highlight: rgb(0x33, 0x39, 0x40),
    fg: rgb(0xad, 0xba, 0xc7),
    fg_dark: rgb(0x91, 0x9d, 0xa8),
    comment: rgb(0x76, 0x83, 0x90),
    blue: rgb(0x53, 0x9b, 0xf5),
    cyan: rgb(0x56, 0xd4, 0xdd),
    magenta: rgb(0xb0, 0x83, 0xf0),
    green: rgb(0x57, 0xab, 0x5a),
    yellow: rgb(0xc6, 0x90, 0x26),
    red: rgb(0xf4, 0x70, 0x67),
};

pub const MATERIAL_DARK: Theme = Theme {
    name: Cow::Borrowed("material-dark"),
    kind: ThemeKind::Dark,
    bg: rgb(0x12, 0x12, 0x12),
    bg_highlight: rgb(0x2b, 0x2b, 0x2b),
    fg: rgb(0xe0, 0xe0, 0xe0),
    fg_dark: rgb(0xb7, 0xb7, 0xb7),
    comment: rgb(0x9a, 0xa0, 0xa6),
    blue: rgb(0x8a, 0xb4, 0xf8),
    cyan: rgb(0x78, 0xd9, 0xec),
    magenta: rgb(0xd7, 0xae, 0xfb),
    green: rgb(0x81, 0xc9, 0x95),
    yellow: rgb(0xfd, 0xd6, 0x63),
    red: rgb(0xf2, 0x8b, 0x82),
};

pub const SHADES_OF_PURPLE: Theme = Theme {
    name: Cow::Borrowed("shades-of-purple"),
    kind: ThemeKind::Dark,
    bg: rgb(0x2d, 0x2b, 0x55),
    bg_highlight: rgb(0x46, 0x44, 0x69),
    fg: rgb(0xff, 0xff, 0xff),
    fg_dark: rgb(0xd5, 0xd5, 0xdd),
    comment: rgb(0xa5, 0x99, 0xe9),
    blue: rgb(0x69, 0x43, 0xff),
    cyan: rgb(0x80, 0xfc, 0xff),
    magenta: rgb(0xff, 0x62, 0x8c),
    green: rgb(0x3a, 0xd9, 0x00),
    yellow: rgb(0xfa, 0xd0, 0x00),
    red: rgb(0xec, 0x3a, 0x37),
};

pub const XCODE_DARK: Theme = Theme {
    name: Cow::Borrowed("xcode-dark"),
    kind: ThemeKind::Dark,
    bg: rgb(0x29, 0x2a, 0x30),
    bg_highlight: rgb(0x43, 0x44, 0x49),
    fg: rgb(0xff, 0xff, 0xff),
    fg_dark: rgb(0xd4, 0xd4, 0xd6),
    comment: rgb(0x6c, 0x79, 0x86),
    blue: rgb(0x5d, 0xd8, 0xff),
    cyan: rgb(0xa1, 0x67, 0xe6),
    magenta: rgb(0xfc, 0x5f, 0xa3),
    green: rgb(0x41, 0xa1, 0xc0),
    yellow: rgb(0xd0, 0xbf, 0x69),
    red: rgb(0xfc, 0x6a, 0x5d),
};

pub const MONOKAI: Theme = Theme {
    name: Cow::Borrowed("monokai"),
    kind: ThemeKind::Dark,
    bg: rgb(0x27, 0x28, 0x22),
    bg_highlight: rgb(0x40, 0x41, 0x3b),
    fg: rgb(0xf8, 0xf8, 0xf2),
    fg_dark: rgb(0xce, 0xce, 0xc8),
    comment: rgb(0x75, 0x71, 0x5e),
    blue: rgb(0x66, 0xd9, 0xef),
    cyan: rgb(0xa1, 0xef, 0xe4),
    magenta: rgb(0xae, 0x81, 0xff),
    green: rgb(0xa6, 0xe2, 0x2e),
    yellow: rgb(0xe6, 0xdb, 0x74),
    red: rgb(0xf9, 0x26, 0x72),
};

pub const VITESSE_LIGHT: Theme = Theme {
    name: Cow::Borrowed("vitesse-light"),
    kind: ThemeKind::Light,
    bg: rgb(0xff, 0xff, 0xff),
    bg_highlight: rgb(0xe7, 0xe7, 0xe7),
    fg: rgb(0x39, 0x3a, 0x34),
    fg_dark: rgb(0x61, 0x61, 0x5d),
    comment: rgb(0xa0, 0xad, 0xa0),
    blue: rgb(0x29, 0x6a, 0xa3),
    cyan: rgb(0x2f, 0x79, 0x8a),
    magenta: rgb(0xa1, 0x38, 0x65),
    green: rgb(0x1e, 0x75, 0x4f),
    yellow: rgb(0xbd, 0xa4, 0x37),
    red: rgb(0xab, 0x59, 0x59),
};

pub const VSCODE_LIGHT_MODERN: Theme = Theme {
    name: Cow::Borrowed("vscode-light-modern"),
    kind: ThemeKind::Light,
    bg: rgb(0xff, 0xff, 0xff),
    bg_highlight: rgb(0xe7, 0xe7, 0xe7),
    fg: rgb(0x3b, 0x3b, 0x3b),
    fg_dark: rgb(0x62, 0x62, 0x62),
    comment: rgb(0x73, 0x73, 0x73),
    blue: rgb(0x00, 0x00, 0xff),
    cyan: rgb(0x26, 0x7f, 0x99),
    magenta: rgb(0xaf, 0x00, 0xdb),
    green: rgb(0x00, 0x80, 0x00),
    yellow: rgb(0x79, 0x5e, 0x26),
    red: rgb(0xcd, 0x31, 0x31),
};

pub const KANAGAWA_LOTUS: Theme = Theme {
    name: Cow::Borrowed("kanagawa-lotus"),
    kind: ThemeKind::Light,
    bg: rgb(0xf2, 0xec, 0xbc),
    bg_highlight: rgb(0xdf, 0xda, 0xb1),
    fg: rgb(0x54, 0x54, 0x64),
    fg_dark: rgb(0x74, 0x72, 0x76),
    comment: rgb(0x8a, 0x89, 0x80),
    blue: rgb(0x4d, 0x69, 0x9b),
    cyan: rgb(0x59, 0x7b, 0x75),
    magenta: rgb(0xb3, 0x5b, 0x79),
    green: rgb(0x6f, 0x89, 0x4e),
    yellow: rgb(0x77, 0x71, 0x3f),
    red: rgb(0xc8, 0x40, 0x53),
};

pub const MATERIAL_LIGHT: Theme = Theme {
    name: Cow::Borrowed("material-light"),
    kind: ThemeKind::Light,
    bg: rgb(0xff, 0xff, 0xff),
    bg_highlight: rgb(0xe4, 0xe4, 0xe5),
    fg: rgb(0x20, 0x21, 0x24),
    fg_dark: rgb(0x4d, 0x4d, 0x50),
    comment: rgb(0x5f, 0x63, 0x68),
    blue: rgb(0x1a, 0x73, 0xe8),
    cyan: rgb(0x00, 0x7b, 0x83),
    magenta: rgb(0x93, 0x34, 0xe6),
    green: rgb(0x18, 0x80, 0x38),
    yellow: rgb(0xea, 0x86, 0x00),
    red: rgb(0xd9, 0x30, 0x25),
};

pub const ALUCARD: Theme = Theme {
    name: Cow::Borrowed("alucard"),
    kind: ThemeKind::Light,
    bg: rgb(0xff, 0xfb, 0xeb),
    bg_highlight: rgb(0xe4, 0xe1, 0xd3),
    fg: rgb(0x1f, 0x1f, 0x1f),
    fg_dark: rgb(0x4c, 0x4b, 0x48),
    comment: rgb(0x6c, 0x66, 0x4b),
    blue: rgb(0x64, 0x4a, 0xc9),
    cyan: rgb(0x03, 0x6a, 0x96),
    magenta: rgb(0xa3, 0x14, 0x4d),
    green: rgb(0x14, 0x71, 0x0a),
    yellow: rgb(0x84, 0x6e, 0x15),
    red: rgb(0xcb, 0x3a, 0x2a),
};

pub const XCODE_LIGHT: Theme = Theme {
    name: Cow::Borrowed("xcode-light"),
    kind: ThemeKind::Light,
    bg: rgb(0xff, 0xff, 0xff),
    bg_highlight: rgb(0xe5, 0xe5, 0xe5),
    fg: rgb(0x26, 0x26, 0x26),
    fg_dark: rgb(0x51, 0x51, 0x51),
    comment: rgb(0x8a, 0x99, 0xa6),
    blue: rgb(0x27, 0x2a, 0xd8),
    cyan: rgb(0x0f, 0x68, 0xa0),
    magenta: rgb(0xad, 0x3d, 0xa4),
    green: rgb(0x00, 0x74, 0x00),
    yellow: rgb(0x78, 0x49, 0x2a),
    red: rgb(0xc4, 0x1a, 0x16),
};

pub const MONOKAI_PRO_LIGHT: Theme = Theme {
    name: Cow::Borrowed("monokai-pro-light"),
    kind: ThemeKind::Light,
    bg: rgb(0xfa, 0xf4, 0xf2),
    bg_highlight: rgb(0xe1, 0xdb, 0xda),
    fg: rgb(0x29, 0x24, 0x2a),
    fg_dark: rgb(0x53, 0x4e, 0x52),
    comment: rgb(0xa5, 0x9f, 0xa0),
    blue: rgb(0x1c, 0x8c, 0xa8),
    cyan: rgb(0x1c, 0x8c, 0xa8),
    magenta: rgb(0x70, 0x58, 0xbe),
    green: rgb(0x26, 0x9d, 0x69),
    yellow: rgb(0xcc, 0x7a, 0x0a),
    red: rgb(0xe1, 0x47, 0x75),
};

pub const BUILTINS: &[Theme] = &[
    // Dark
    GITHUB_DARK,
    CATPPUCCIN,
    GRUVBOX,
    ROSE_PINE,
    EVERFOREST,
    VITESSE_DARK,
    TOKYONIGHT,
    TOKYONIGHT_STORM,
    VSCODE_DARK_MODERN,
    KANAGAWA,
    GITHUB_DARK_DIMMED,
    MATERIAL_DARK,
    DRACULA,
    SHADES_OF_PURPLE,
    XCODE_DARK,
    MONOKAI,
    ONE_DARK,
    // Light
    GITHUB_LIGHT,
    CATPPUCCIN_LATTE,
    GRUVBOX_LIGHT,
    ROSE_PINE_DAWN,
    EVERFOREST_LIGHT,
    VITESSE_LIGHT,
    TOKYONIGHT_DAY,
    VSCODE_LIGHT_MODERN,
    KANAGAWA_LOTUS,
    MATERIAL_LIGHT,
    ALUCARD,
    XCODE_LIGHT,
    MONOKAI_PRO_LIGHT,
    ONE_LIGHT,
];

/// The active theme, behind an `Arc` so readers can snapshot it without
/// holding the lock. Every mutation must go through [`store_active`] so the
/// generation counter stays in sync with the contents.
static ACTIVE: LazyLock<RwLock<Arc<Theme>>> = LazyLock::new(|| RwLock::new(Arc::new(TOKYONIGHT)));

/// Bumped on every write to [`ACTIVE`]. Per-thread caches compare against it
/// to decide whether their snapshot is stale, so the steady-state cost of a
/// theme read is one relaxed atomic load plus a thread-local access — no lock
/// at all. Views call color tokens thousands of times per frame, so this is
/// the difference between per-token lock traffic and none.
static GENERATION: AtomicU64 = AtomicU64::new(0);

thread_local! {
    /// This thread's `(generation, snapshot)` cache of [`ACTIVE`].
    static ACTIVE_CACHE: RefCell<Option<(u64, Arc<Theme>)>> = const { RefCell::new(None) };
}

fn store_active(theme: Theme) {
    *ACTIVE.write().unwrap_or_else(|e| e.into_inner()) = Arc::new(theme);
    // Release pairs with the Acquire load in `active()`: a thread that sees
    // the new generation must also see the new `Arc` through the lock.
    GENERATION.fetch_add(1, Ordering::Release);
}

/// A snapshot of the active theme, served from this thread's cache when the
/// generation is unchanged (the overwhelmingly common case).
fn active() -> Arc<Theme> {
    let gen_now = GENERATION.load(Ordering::Acquire);
    ACTIVE_CACHE.with(|cell| {
        if let Some((cached_gen, theme)) = &*cell.borrow() {
            if *cached_gen == gen_now {
                return theme.clone();
            }
        }
        let theme = ACTIVE.read().unwrap_or_else(|e| e.into_inner()).clone();
        *cell.borrow_mut() = Some((gen_now, theme.clone()));
        theme
    })
}

/// User-defined themes loaded from `themes.json` (see `theme_file`). Empty
/// until `load_custom()` is called (typically once at startup).
static CUSTOM: RwLock<Vec<Theme>> = RwLock::new(Vec::new());

/// Serializes any test, in any module, that touches the shared `CUSTOM`
/// registry (and, via `save_custom`, the on-disk `themes.json`) so parallel
/// `cargo test` runs don't stomp on each other. Not `#[cfg(test)]`: it's
/// shared by tests in the downstream `grove` crate, which can't see this
/// crate's `cfg(test)` items, so it must be compiled unconditionally. An
/// uncontended `Mutex<()>` static costs nothing in release builds.
pub static CUSTOM_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub fn current() -> Theme {
    (*active()).clone()
}

/// Borrow the active theme without cloning it — prefer this on hot paths
/// (e.g. per-frame palette color lookups) over `current()`.
pub fn with_current<R>(f: impl FnOnce(&Theme) -> R) -> R {
    f(&active())
}

/// Look up a theme by name without touching the global `ACTIVE` theme.
/// Checks builtins first, then custom themes (builtins win on collision).
/// Used to resolve a project's pinned "Project theme" for PTY rendering; an
/// unknown/stale name yields `None` so callers fall back to the global theme.
pub fn by_name(name: &str) -> Option<Theme> {
    if let Some(t) = BUILTINS.iter().find(|t| t.name == name) {
        return Some(t.clone());
    }
    CUSTOM
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .find(|t| t.name == name)
        .cloned()
}

pub fn set_by_name(name: &str) -> bool {
    if let Some(t) = by_name(name) {
        store_active(t);
        true
    } else {
        false
    }
}

pub fn set(theme: Theme) {
    store_active(theme);
}

pub fn themes_of(kind: ThemeKind) -> Vec<Theme> {
    let mut v: Vec<Theme> = BUILTINS
        .iter()
        .filter(|t| t.kind == kind)
        .cloned()
        .collect();
    v.sort_by(|a, b| a.name.cmp(&b.name));
    v
}

/// The single source of truth for "every theme of `kind` a user can pick
/// from", in the stable order every selection surface must agree on:
/// builtins first (alphabetical, `themes_of`'s order), then custom themes
/// (alphabetical, `custom_themes_of`'s order) — never interleaved. This
/// mirrors the palette's `theme_pane_combined_rows`, which is the reference
/// implementation this function generalizes.
///
/// Any UI that stores a selection as a positional index into "the list of
/// themes" (rather than resolving by name) must build that list — and
/// compute that index — via this function, not `themes_of` alone, or the
/// index will point at the wrong theme the moment a custom theme exists.
pub fn selectable_themes_of(kind: ThemeKind) -> Vec<Theme> {
    let mut v = themes_of(kind);
    v.extend(custom_themes_of(kind));
    v
}

/// The theme at position `idx` in `selectable_themes_of(kind)`, without
/// materializing that whole list. Same order, same result as
/// `selectable_themes_of(kind).get(idx).cloned()` — this exists purely for
/// hot paths (per-frame render code) where building ~40 themes, taking the
/// `CUSTOM` lock and sorting on every call is pure waste.
pub fn selectable_theme_at(kind: ThemeKind, idx: usize) -> Option<Theme> {
    /// `BUILTINS` of each kind in `themes_of`'s order, sorted once. The set is
    /// a compile-time constant, so the order can never change at runtime.
    static SORTED_BUILTINS: std::sync::OnceLock<[Vec<&'static Theme>; 2]> =
        std::sync::OnceLock::new();
    let sorted = SORTED_BUILTINS.get_or_init(|| {
        [ThemeKind::Dark, ThemeKind::Light].map(|k| {
            let mut v: Vec<&'static Theme> = BUILTINS.iter().filter(|t| t.kind == k).collect();
            v.sort_by(|a, b| a.name.cmp(&b.name));
            v
        })
    });
    let builtins = match kind {
        ThemeKind::Dark => &sorted[0],
        ThemeKind::Light => &sorted[1],
    };
    match builtins.get(idx) {
        Some(t) => Some((*t).clone()),
        None => custom_themes_of(kind).get(idx - builtins.len()).cloned(),
    }
}

/// Whether `name` refers to a builtin theme.
fn is_builtin(name: &str) -> bool {
    BUILTINS.iter().any(|t| t.name == name)
}

/// Whether `name` refers to a custom (user-defined) theme.
pub fn is_custom(name: &str) -> bool {
    CUSTOM
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .any(|t| t.name == name)
}

/// Custom themes of `kind`, sorted alphabetically. UI shows these separately
/// from `themes_of` (builtins-only).
pub fn custom_themes_of(kind: ThemeKind) -> Vec<Theme> {
    let mut v: Vec<Theme> = CUSTOM
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .filter(|t| t.kind == kind)
        .cloned()
        .collect();
    v.sort_by(|a, b| a.name.cmp(&b.name));
    v
}

/// All custom themes (either kind), sorted alphabetically — used by
/// `Modal::ThemeManager`'s flat list, which shows a kind badge per row
/// instead of splitting into Dark/Light tabs like the palette's Theme pane.
pub fn all_custom_themes() -> Vec<Theme> {
    let mut v: Vec<Theme> = CUSTOM.read().unwrap_or_else(|e| e.into_inner()).clone();
    v.sort_by(|a, b| a.name.cmp(&b.name));
    v
}

/// Loads `themes.json` into `CUSTOM`, replacing its previous contents.
/// Returns the list of entries that were skipped, with human-readable
/// reasons. A missing file is not an error (yields an empty list, no
/// errors); a corrupt top-level file yields a single error and leaves
/// `CUSTOM` empty without touching the file on disk.
pub fn load_custom() -> Vec<crate::theme_file::ThemeLoadError> {
    let (themes, errors) = crate::theme_file::load();
    *CUSTOM.write().unwrap_or_else(|e| e.into_inner()) = themes;
    errors
}

/// Persists the current `CUSTOM` contents to `themes.json`.
pub fn save_custom() -> std::io::Result<()> {
    let themes = CUSTOM.read().unwrap_or_else(|e| e.into_inner()).clone();
    crate::theme_file::save(&themes)
}

/// Adds a new custom theme. Rejects a name that collides with a builtin or
/// an existing custom theme.
pub fn add_custom(mut theme: Theme) -> Result<(), String> {
    let trimmed = theme.name.trim().to_string();
    if trimmed.is_empty() {
        return Err("name can't be empty".to_string());
    }
    theme.name = Cow::Owned(trimmed);
    if is_builtin(&theme.name) {
        return Err(format!("\"{}\" shadows a built-in theme", theme.name));
    }
    let mut guard = CUSTOM.write().unwrap_or_else(|e| e.into_inner());
    if guard.iter().any(|t| t.name == theme.name) {
        return Err(format!(
            "a custom theme named \"{}\" already exists",
            theme.name
        ));
    }
    guard.push(theme);
    drop(guard);
    save_custom().map_err(|e| e.to_string())
}

/// Replaces the custom theme named `original_name` with `theme` in place
/// (preserving its position). Errors if `original_name` isn't a known
/// custom theme, if `theme.name` is empty/whitespace-only (mirrors the
/// list-view rename path's validation — a theme can never be saved with a
/// blank name), or if renaming to `theme.name` would collide.
pub fn update_custom(original_name: &str, mut theme: Theme) -> Result<(), String> {
    let trimmed = theme.name.trim().to_string();
    if trimmed.is_empty() {
        return Err("name can't be empty".to_string());
    }
    theme.name = Cow::Owned(trimmed);
    let mut guard = CUSTOM.write().unwrap_or_else(|e| e.into_inner());
    let idx = guard
        .iter()
        .position(|t| t.name == original_name)
        .ok_or_else(|| format!("no custom theme named \"{original_name}\""))?;
    if theme.name != original_name {
        if is_builtin(&theme.name) {
            return Err(format!("\"{}\" shadows a built-in theme", theme.name));
        }
        if guard.iter().any(|t| t.name == theme.name) {
            return Err(format!(
                "a custom theme named \"{}\" already exists",
                theme.name
            ));
        }
    }
    guard[idx] = theme;
    drop(guard);
    save_custom().map_err(|e| e.to_string())
}

/// Renames a custom theme, updating `ACTIVE` in place if it was the active
/// theme (nothing else is affected — callers own any project-pin bookkeeping).
pub fn rename_custom(old: &str, new: &str) -> Result<(), String> {
    if old == new {
        return Ok(());
    }
    if is_builtin(new) {
        return Err(format!("\"{new}\" shadows a built-in theme"));
    }
    let mut guard = CUSTOM.write().unwrap_or_else(|e| e.into_inner());
    if guard.iter().any(|t| t.name == new) {
        return Err(format!("a custom theme named \"{new}\" already exists"));
    }
    let idx = guard
        .iter()
        .position(|t| t.name == old)
        .ok_or_else(|| format!("no custom theme named \"{old}\""))?;
    guard[idx].name = Cow::Owned(new.to_string());
    drop(guard);
    save_custom().map_err(|e| e.to_string())?;

    let current_active = active();
    if current_active.name == old {
        let mut renamed = (*current_active).clone();
        renamed.name = Cow::Owned(new.to_string());
        store_active(renamed);
    }
    Ok(())
}

/// Removes a custom theme by name. Returns `false` if no such theme exists.
pub fn delete_custom(name: &str) -> bool {
    let mut guard = CUSTOM.write().unwrap_or_else(|e| e.into_inner());
    let before = guard.len();
    guard.retain(|t| t.name != name);
    let removed = guard.len() != before;
    drop(guard);
    if removed {
        if let Err(e) = save_custom() {
            tracing::warn!(error = format!("{e:#}"), "failed to save custom themes");
        }
    }
    removed
}

/// Produces a fresh, non-colliding name derived from `base`: "X copy", then
/// "X copy 2", "X copy 3", ... — checked against both builtins and customs.
pub fn duplicate_name(base: &str) -> String {
    let exists = |name: &str| {
        BUILTINS.iter().any(|t| t.name == name)
            || CUSTOM
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .any(|t| t.name == name)
    };
    let first = format!("{base} copy");
    if !exists(&first) {
        return first;
    }
    let mut n = 2;
    loop {
        let candidate = format!("{base} copy {n}");
        if !exists(&candidate) {
            return candidate;
        }
        n += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(name: &str, kind: ThemeKind) -> Theme {
        Theme {
            name: Cow::Owned(name.to_string()),
            kind,
            bg: rgb(0, 0, 0),
            bg_highlight: rgb(0, 0, 0),
            fg: rgb(0, 0, 0),
            fg_dark: rgb(0, 0, 0),
            comment: rgb(0, 0, 0),
            blue: rgb(0, 0, 0),
            cyan: rgb(0, 0, 0),
            magenta: rgb(0, 0, 0),
            green: rgb(0, 0, 0),
            yellow: rgb(0, 0, 0),
            red: rgb(0, 0, 0),
        }
    }

    /// Snapshots `CUSTOM`'s contents and restores them (in-memory only, no
    /// disk write) when dropped, so a test's mutations never leak into
    /// another test even under panics.
    struct CustomGuard {
        original: Vec<Theme>,
    }

    impl CustomGuard {
        fn new() -> Self {
            let original = CUSTOM.read().unwrap_or_else(|e| e.into_inner()).clone();
            Self { original }
        }
    }

    impl Drop for CustomGuard {
        fn drop(&mut self) {
            *CUSTOM.write().unwrap_or_else(|e| e.into_inner()) = std::mem::take(&mut self.original);
        }
    }

    #[test]
    fn selectable_themes_of_lists_builtins_then_customs() {
        let _lock = CUSTOM_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _guard = CustomGuard::new();
        *CUSTOM.write().unwrap_or_else(|e| e.into_inner()) = vec![
            sample("zz-custom-dark-b", ThemeKind::Dark),
            sample("zz-custom-dark-a", ThemeKind::Dark),
            sample("zz-custom-light-only", ThemeKind::Light),
        ];

        let names = |v: &[Theme]| v.iter().map(|t| t.name.to_string()).collect::<Vec<_>>();
        let builtins = themes_of(ThemeKind::Dark);
        let customs = custom_themes_of(ThemeKind::Dark);
        let selectable = selectable_themes_of(ThemeKind::Dark);

        // Every builtin comes first, in `themes_of`'s order, followed by
        // every custom of the same kind, in `custom_themes_of`'s order —
        // never interleaved.
        assert_eq!(selectable.len(), builtins.len() + customs.len());
        assert_eq!(names(&selectable[..builtins.len()]), names(&builtins));
        assert_eq!(names(&selectable[builtins.len()..]), names(&customs));
        // Customs are alphabetical among themselves.
        assert_eq!(customs[0].name, "zz-custom-dark-a");
        assert_eq!(customs[1].name, "zz-custom-dark-b");

        // A custom theme of the other kind never leaks into this kind's list.
        assert!(!selectable.iter().any(|t| t.name == "zz-custom-light-only"));
    }

    #[test]
    fn selectable_themes_of_filters_by_kind() {
        let _lock = CUSTOM_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _guard = CustomGuard::new();
        *CUSTOM.write().unwrap_or_else(|e| e.into_inner()) = vec![
            sample("zz-custom-dark-only", ThemeKind::Dark),
            sample("zz-custom-light-only", ThemeKind::Light),
        ];

        let dark = selectable_themes_of(ThemeKind::Dark);
        let light = selectable_themes_of(ThemeKind::Light);
        assert!(dark.iter().all(|t| t.kind == ThemeKind::Dark));
        assert!(light.iter().all(|t| t.kind == ThemeKind::Light));
        assert!(dark.iter().any(|t| t.name == "zz-custom-dark-only"));
        assert!(light.iter().any(|t| t.name == "zz-custom-light-only"));
    }

    #[test]
    fn selectable_themes_of_is_builtins_only_when_no_customs() {
        let _lock = CUSTOM_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _guard = CustomGuard::new();
        *CUSTOM.write().unwrap_or_else(|e| e.into_inner()) = Vec::new();
        let names = |v: &[Theme]| v.iter().map(|t| t.name.to_string()).collect::<Vec<_>>();
        assert_eq!(
            names(&selectable_themes_of(ThemeKind::Dark)),
            names(&themes_of(ThemeKind::Dark))
        );
    }

    #[test]
    fn duplicate_name_sequence() {
        let _lock = CUSTOM_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _guard = CustomGuard::new();
        *CUSTOM.write().unwrap_or_else(|e| e.into_inner()) = vec![
            sample("mytheme copy", ThemeKind::Dark),
            sample("mytheme copy 2", ThemeKind::Dark),
        ];
        assert_eq!(duplicate_name("mytheme"), "mytheme copy 3");

        *CUSTOM.write().unwrap_or_else(|e| e.into_inner()) = Vec::new();
        assert_eq!(duplicate_name("mytheme"), "mytheme copy");

        // A duplicate name that collides with a builtin is also avoided.
        let builtin_name = BUILTINS[0].name.to_string();
        assert_eq!(
            duplicate_name(&builtin_name),
            format!("{builtin_name} copy")
        );
    }

    #[test]
    fn load_custom_replaces_registry_contents_rather_than_merging() {
        let _lock = CUSTOM_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _guard = CustomGuard::new();
        // An in-memory-only entry that was never written to `themes.json`.
        *CUSTOM.write().unwrap_or_else(|e| e.into_inner()) =
            vec![sample("stale-injected-not-on-disk", ThemeKind::Dark)];
        let _ = load_custom();
        // Reloading re-reads from disk: a genuine replace throws this stale
        // in-memory-only entry away rather than keeping it alongside
        // whatever was actually loaded.
        assert!(
            !CUSTOM
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .any(|t| t.name == "stale-injected-not-on-disk"),
            "load_custom must replace CUSTOM's contents, not merge into them"
        );
    }

    #[test]
    fn rename_custom_rejects_collision_with_existing_custom_and_builtin() {
        let _lock = CUSTOM_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _guard = CustomGuard::new();
        *CUSTOM.write().unwrap_or_else(|e| e.into_inner()) = vec![
            sample("theme-a", ThemeKind::Dark),
            sample("theme-b", ThemeKind::Dark),
        ];

        // Colliding with another custom theme is rejected; nothing changes.
        let err = rename_custom("theme-a", "theme-b");
        assert!(err.is_err());
        assert_eq!(
            CUSTOM.read().unwrap_or_else(|e| e.into_inner())[0].name,
            "theme-a"
        );

        // Colliding with a builtin is rejected too.
        let builtin_name = BUILTINS[0].name.to_string();
        let err = rename_custom("theme-a", &builtin_name);
        assert!(err.is_err());
    }

    #[test]
    fn add_custom_rejects_empty_or_whitespace_name() {
        let _lock = CUSTOM_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _guard = CustomGuard::new();
        *CUSTOM.write().unwrap_or_else(|e| e.into_inner()) = Vec::new();

        assert!(add_custom(sample("", ThemeKind::Dark)).is_err());
        assert!(add_custom(sample("   ", ThemeKind::Dark)).is_err());
        // Rejected before ever touching the registry.
        assert!(CUSTOM.read().unwrap_or_else(|e| e.into_inner()).is_empty());
    }

    #[test]
    fn update_custom_rejects_empty_or_whitespace_name() {
        let _lock = CUSTOM_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _guard = CustomGuard::new();
        *CUSTOM.write().unwrap_or_else(|e| e.into_inner()) =
            vec![sample("theme-a", ThemeKind::Dark)];

        let err = update_custom("theme-a", sample("   ", ThemeKind::Dark));
        assert!(err.is_err());
        // The original entry is left untouched by the rejected save.
        assert_eq!(
            CUSTOM.read().unwrap_or_else(|e| e.into_inner())[0].name,
            "theme-a"
        );
    }

    #[test]
    fn is_builtin_and_by_name_prefer_builtins_over_customs() {
        let _lock = CUSTOM_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _guard = CustomGuard::new();
        let builtin_name = BUILTINS[0].name.to_string();
        assert!(is_builtin(&builtin_name));
        // A "custom" theme that (illegally) shares a builtin's name still
        // never wins the lookup — builtins are checked first.
        *CUSTOM.write().unwrap_or_else(|e| e.into_inner()) =
            vec![sample(&builtin_name, ThemeKind::Light)];
        let found = by_name(&builtin_name).expect("by_name finds the builtin");
        assert!(matches!(found.kind, k if k == BUILTINS[0].kind));
    }

    #[test]
    fn field_and_set_field_round_trip_every_index_in_field_names_order() {
        let mut t = sample("roundtrip", ThemeKind::Dark);
        for (i, _) in FIELD_NAMES.iter().enumerate() {
            let c = rgb(i as u8, (i * 2) as u8, (i * 3) as u8);
            t.set_field(i, c);
            assert_eq!(
                t.field(i),
                c,
                "field {i} ({}) didn't round-trip",
                FIELD_NAMES[i]
            );
        }
    }

    #[test]
    fn colors_eq_detects_any_single_field_difference() {
        let a = sample("a", ThemeKind::Dark);
        let mut b = a.clone();
        assert!(a.colors_eq(&b), "identical theme must compare equal");
        b.set_field(7, rgb(1, 2, 3)); // magenta
        assert!(!a.colors_eq(&b), "a differing field must break equality");
    }

    #[test]
    fn contrast_ratio_black_on_white_is_maximal() {
        let ratio = contrast_ratio(rgb(0, 0, 0), rgb(255, 255, 255));
        assert!((ratio - 21.0).abs() < 0.01, "black/white ratio was {ratio}");
    }

    #[test]
    fn contrast_ratio_identical_colors_is_one() {
        let ratio = contrast_ratio(rgb(0x33, 0x66, 0x99), rgb(0x33, 0x66, 0x99));
        assert!(
            (ratio - 1.0).abs() < 0.001,
            "identical-color ratio was {ratio}"
        );
    }

    #[test]
    fn contrast_ratio_is_order_independent() {
        let a = rgb(0x10, 0x20, 0x30);
        let b = rgb(0xe0, 0xd0, 0xc0);
        assert_eq!(contrast_ratio(a, b), contrast_ratio(b, a));
    }

    #[test]
    fn contrast_ratio_flags_low_contrast_pair_below_thresholds() {
        // Two similarly dark grays: well under both the amber (4.5) and red
        // (3.0) editor thresholds.
        let ratio = contrast_ratio(rgb(0x20, 0x20, 0x20), rgb(0x30, 0x30, 0x30));
        assert!(ratio < 3.0, "expected a low-contrast pair, got {ratio}");
    }

    #[test]
    fn contrast_partner_indices_are_in_range_and_bg_has_none() {
        assert_eq!(CONTRAST_PARTNER[0], None, "bg has no contrast partner");
        for (i, partner) in CONTRAST_PARTNER.iter().enumerate().skip(1) {
            let p = partner.expect("every non-bg field has a contrast partner");
            assert!(
                p < FIELD_NAMES.len(),
                "partner index {p} for field {i} out of range"
            );
        }
    }
}
