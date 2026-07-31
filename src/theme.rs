//! GUI color tokens, derived live from the active [`grove_core::theme`].
//!
//! A direct port of `src/gui/palette.rs` — same function names, same blend
//! ratios, same dark/light branches — returning `gpui::Hsla` instead of
//! `iced::Color`. Matching names are what keep the Plan 04-07 ports mechanical
//! and reviewable side-by-side against the iced originals.
//!
//! The shared theme exposes a flat `Theme` with `bg / bg_highlight / fg /
//! fg_dark / comment` plus six accents. The GUI uses a richer surface
//! vocabulary (rail, strip, hover, two border weights), so the missing tokens
//! are synthesized by blending the base theme colors at fixed ratios.
//!
//! All accessors read the active theme on each call, so swapping themes at
//! runtime takes effect on the next frame. Reads go through
//! `theme::with_current`, which serves a per-thread snapshot guarded by a
//! generation counter — a token call costs an atomic load, not a lock.
//!
//! **Blending happens component-wise on 0..1 sRGB floats (`gpui::Rgba`),
//! exactly as `palette.rs` does it, and only converts to `Hsla` at the end of
//! each token function.** Blending in HSL space would shift hues and visibly
//! change ~40 themes at once.

#![allow(non_snake_case)]
// The full token vocabulary is ported in one go so the Plan 04-07 region ports
// are mechanical; most tokens have no caller until their region lands.
#![allow(dead_code)]

use gpui::{BorrowAppContext as _, Hsla, Rgba, WindowAppearance};
use grove_core::theme;

/// Grove's defaults when the store names no theme (`src/app/theme_picker.rs:8-9`).
pub const DEFAULT_DARK_THEME: &str = "tokyonight-storm";
pub const DEFAULT_LIGHT_THEME: &str = "tokyonight-day";

const BLACK: Rgba = Rgba {
    r: 0.0,
    g: 0.0,
    b: 0.0,
    a: 1.0,
};

/// grove-core `Color` → sRGB floats. Public so a theme editor can render
/// arbitrary draft-theme swatches without going through `theme::current()`.
pub fn ic(c: theme::Color) -> Rgba {
    match c {
        theme::Color::Rgb(r, g, b) => Rgba {
            r: r as f32 / 255.0,
            g: g as f32 / 255.0,
            b: b as f32 / 255.0,
            a: 1.0,
        },
    }
}

/// Component-wise lerp on sRGB floats — never in HSL space (see module docs).
fn mix(a: Rgba, b: Rgba, t: f32) -> Rgba {
    let t = t.clamp(0.0, 1.0);
    Rgba {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: a.a,
    }
}

fn alpha(c: Rgba, a: f32) -> Rgba {
    Rgba { a, ..c }
}

fn base_bg() -> Rgba {
    theme::with_current(|t| ic(t.bg))
}
fn base_fg() -> Rgba {
    theme::with_current(|t| ic(t.fg))
}

// ── surfaces ─────────────────────────────────────────────────────────────

pub fn BG() -> Hsla {
    base_bg().into()
}

/// Rail / sidebar — slightly darker than BG on dark themes, slightly
/// off-white on light themes.
pub fn BG_RAIL() -> Hsla {
    theme::with_current(|t| {
        let d = if is_dark_of(t) { 0.18 } else { 0.04 };
        mix(ic(t.bg), BLACK, d)
    })
    .into()
}

/// Outer strip / chrome edge — darker than rail.
pub fn BG_STRIP() -> Hsla {
    theme::with_current(bg_strip_of).into()
}

/// Hover surface — partway between bg and bg_highlight.
pub fn BG_HOVER() -> Hsla {
    theme::with_current(bg_hover_of).into()
}

/// Active / selected row.
pub fn BG_HL() -> Hsla {
    theme::with_current(|t| ic(t.bg_highlight)).into()
}

pub fn BORDER() -> Hsla {
    theme::with_current(border_of).into()
}
pub fn BORDER_SOFT() -> Hsla {
    theme::with_current(|t| mix(ic(t.bg), ic(t.fg), 0.07)).into()
}

// ── overlays ─────────────────────────────────────────────────────────────

/// Modal scrim: a translucent wash derived from the theme rather than a
/// fixed black. Dark themes dim toward black; light themes dim toward the
/// foreground so the wash stays visible on near-white backgrounds.
pub fn SCRIM() -> Hsla {
    theme::with_current(|t| {
        let toward = if is_dark_of(t) { BLACK } else { ic(t.fg) };
        alpha(mix(ic(t.bg), toward, 0.9), 0.16)
    })
    .into()
}

// ── text ─────────────────────────────────────────────────────────────────

pub fn FG() -> Hsla {
    base_fg().into()
}
pub fn FG_DIM() -> Hsla {
    theme::with_current(|t| ic(t.fg_dark)).into()
}
pub fn FG_MUTE() -> Hsla {
    theme::with_current(|t| ic(t.comment)).into()
}

// ── accents ──────────────────────────────────────────────────────────────

pub fn BLUE() -> Hsla {
    theme::with_current(|t| ic(t.blue)).into()
}
pub fn CYAN() -> Hsla {
    theme::with_current(|t| ic(t.cyan)).into()
}
pub fn MAGENTA() -> Hsla {
    theme::with_current(|t| ic(t.magenta)).into()
}
pub fn GREEN() -> Hsla {
    theme::with_current(|t| ic(t.green)).into()
}
/// Attention amber — the "needs input" accent. Warmer than YELLOW so it
/// reads as a call to action next to green/working.
pub fn AMBER() -> Hsla {
    amber_rgba().into()
}
fn amber_rgba() -> Rgba {
    theme::with_current(|t| mix(ic(t.yellow), ic(t.red), 0.25))
}
pub fn YELLOW() -> Hsla {
    theme::with_current(|t| ic(t.yellow)).into()
}
pub fn RED() -> Hsla {
    theme::with_current(|t| ic(t.red)).into()
}

/// A 16% wash of RED over BG — the active fill for a danger-flavored
/// segmented control (e.g. "skip permissions"), distinct from the neutral
/// `BG_HL()` used by ordinary active segments.
pub fn RED_WASH() -> Hsla {
    theme::with_current(|t| mix(ic(t.red), ic(t.bg), 0.84)).into()
}

// ── selection (focused Miller column) ──────────────────────────────────────
// The launcher's active column marks its selected row with a cyan-tinted
// gradient fill, a cyan ring, and a left accent bar. Derived from the theme's
// cyan so the treatment tracks theme swaps.

fn cyan_rgba() -> Rgba {
    theme::with_current(|t| ic(t.cyan))
}

/// Stronger end of the selected-row gradient (left edge).
pub fn SEL_TINT_STRONG() -> Hsla {
    alpha(cyan_rgba(), 0.22).into()
}
/// Softer end of the selected-row gradient (right edge).
pub fn SEL_TINT_SOFT() -> Hsla {
    alpha(cyan_rgba(), 0.10).into()
}
/// Ring outlining the selected row in the focused column.
pub fn SEL_RING() -> Hsla {
    alpha(cyan_rgba(), 0.5).into()
}

// ── theme-parameterized variants ────────────────────────────────────────────
// Used to render PTY *content* (background fill, default fg, cursor, ANSI
// 0-15) under a per-project "Project theme" override, decoupled from the
// global `theme::current()` that the accessors above read. App chrome always
// uses the plain accessors above and is unaffected by a project's pinned theme.
//
// These return `Rgba` (not `Hsla`) because they are blend inputs to each
// other; callers that paint convert with `.into()`.

fn is_dark_of(t: &theme::Theme) -> bool {
    matches!(t.kind, theme::ThemeKind::Dark)
}

pub fn bg_of(t: &theme::Theme) -> Rgba {
    ic(t.bg)
}
pub fn fg_of(t: &theme::Theme) -> Rgba {
    ic(t.fg)
}
pub fn fg_mute_of(t: &theme::Theme) -> Rgba {
    ic(t.comment)
}
pub fn blue_of(t: &theme::Theme) -> Rgba {
    ic(t.blue)
}
pub fn cyan_of(t: &theme::Theme) -> Rgba {
    ic(t.cyan)
}
pub fn magenta_of(t: &theme::Theme) -> Rgba {
    ic(t.magenta)
}
pub fn green_of(t: &theme::Theme) -> Rgba {
    ic(t.green)
}
pub fn yellow_of(t: &theme::Theme) -> Rgba {
    ic(t.yellow)
}
pub fn red_of(t: &theme::Theme) -> Rgba {
    ic(t.red)
}

/// Themed variant of `BG_STRIP` — used for ANSI color 0 inside PTY content
/// rendered under a per-project override theme.
pub fn bg_strip_of(t: &theme::Theme) -> Rgba {
    let bg = ic(t.bg);
    if is_dark_of(t) {
        mix(bg, BLACK, 0.32)
    } else {
        mix(bg, BLACK, 0.08)
    }
}

/// Themed variant of `BG_HL`.
pub fn bg_hl_of(t: &theme::Theme) -> Rgba {
    ic(t.bg_highlight)
}
/// Themed variant of `BG_HOVER`.
pub fn bg_hover_of(t: &theme::Theme) -> Rgba {
    mix(bg_of(t), bg_hl_of(t), 0.55)
}
/// Themed variant of `BORDER`.
pub fn border_of(t: &theme::Theme) -> Rgba {
    mix(bg_of(t), fg_of(t), 0.16)
}
/// Themed variant of `SEL_RING`.
pub fn sel_ring_of(t: &theme::Theme) -> Rgba {
    alpha(cyan_of(t), 0.5)
}

// ── the Global ───────────────────────────────────────────────────────────

/// Theme *resolution policy* — the colors themselves are not stored here.
/// grove-core's `theme::ACTIVE` remains the single source of truth and the
/// token functions above read it, exactly as the iced app does.
pub struct ThemeState {
    pub follow_system: bool,
    pub dark_name: String,
    pub light_name: String,
    /// Seeded from the window on the first frame, updated on observation.
    pub system_mode: WindowAppearance,
    /// Bumped on every change; Plan 04's terminal element uses it as a
    /// repaint/cache key.
    pub generation: u64,
}

impl gpui::Global for ThemeState {}

impl ThemeState {
    pub fn new(follow_system: bool, dark_name: String, light_name: String) -> Self {
        Self {
            follow_system,
            dark_name,
            light_name,
            system_mode: WindowAppearance::default(),
            generation: 0,
        }
    }

    /// The theme name this appearance resolves to under the current policy.
    /// Port of `resolve_system_theme_name` (`src/app/theme_picker.rs:193-206`):
    /// anything that isn't explicitly light resolves dark.
    pub fn resolve_system_theme_name(&self, mode: WindowAppearance) -> &str {
        match mode {
            WindowAppearance::Light | WindowAppearance::VibrantLight => &self.light_name,
            WindowAppearance::Dark | WindowAppearance::VibrantDark => &self.dark_name,
        }
    }

    /// Applies `name` and bumps the generation. Unknown names are ignored by
    /// grove-core, so the previous theme stays active.
    pub fn set_by_name(cx: &mut gpui::App, name: &str) {
        if grove_core::theme::set_by_name(name) {
            cx.update_global::<Self, _>(|this, _| this.generation += 1);
            cx.refresh_windows();
        }
    }

    /// Re-applies the active theme from `system_mode` when following the OS
    /// setting. No-op otherwise. Port of `apply_system_theme`
    /// (`src/app/theme_picker.rs:210-217`).
    pub fn apply_system_theme(cx: &mut gpui::App) {
        let name = cx.global::<Self>();
        if !name.follow_system {
            return;
        }
        let name = name.resolve_system_theme_name(name.system_mode).to_string();
        Self::set_by_name(cx, &name);
    }

    /// Records a new OS appearance and re-resolves. Called once with the
    /// window's `appearance()` on the first frame — seeding it there rather
    /// than waiting for the first OS notification is why follow-system looks
    /// correct immediately (`src/gui/mod.rs:63-68`).
    pub fn set_system_mode(cx: &mut gpui::App, mode: WindowAppearance) {
        cx.update_global::<Self, _>(|this, _| this.system_mode = mode);
        Self::apply_system_theme(cx);
    }
}

// ── custom-theme management (Plan 08 Task 6 Step 4) ──────────────────────
//
// Every mutation goes through `grove_core::theme_file::save` and then
// `load_custom`, so `theme::CUSTOM` and `themes.json` can never disagree.
// grove-core stays the owner of the file format; nothing here reimplements it.

/// A blank custom theme, as the paste-first editor's starting buffer.
pub fn new_theme_template() -> String {
    let base = theme::by_name(DEFAULT_DARK_THEME)
        .or_else(|| theme::BUILTINS.first().cloned())
        .unwrap_or_else(|| theme::BUILTINS[0].clone());
    let mut draft = base;
    draft.name = std::borrow::Cow::Owned("my theme".to_string());
    grove_core::theme_file::to_named_lines(&draft)
}

fn persist_custom(themes: &[theme::Theme]) -> Result<(), String> {
    grove_core::theme_file::save(themes).map_err(|e| e.to_string())?;
    theme::load_custom();
    Ok(())
}

pub fn delete_custom_theme(name: &str) -> Result<(), String> {
    let mut themes = theme::all_custom_themes();
    themes.retain(|t| t.name != name);
    persist_custom(&themes)
}

pub fn rename_custom_theme(from: &str, to: &str) -> Result<(), String> {
    if to.trim().is_empty() {
        return Err("name required".into());
    }
    let mut themes = theme::all_custom_themes();
    let Some(t) = themes.iter_mut().find(|t| t.name == from) else {
        return Err(format!("'{from}' not found"));
    };
    t.name = std::borrow::Cow::Owned(to.to_string());
    persist_custom(&themes)
}

pub fn duplicate_custom_theme(name: &str) -> Result<(), String> {
    let mut themes = theme::all_custom_themes();
    let Some(src) = themes.iter().find(|t| t.name == name).cloned() else {
        return Err(format!("'{name}' not found"));
    };
    let mut copy = src;
    let mut candidate = format!("{name} copy");
    let mut n = 2;
    while themes.iter().any(|t| t.name == candidate) {
        candidate = format!("{name} copy {n}");
        n += 1;
    }
    copy.name = std::borrow::Cow::Owned(candidate);
    themes.push(copy);
    persist_custom(&themes)
}

/// Save the paste-first editor's buffer. The buffer is `to_named_lines`'
/// output (or a pasted equivalent), so it round-trips through
/// `theme_file::parse_paste` onto a draft derived from the default theme.
pub fn save_custom_theme_json(buffer: &str) -> Result<(), String> {
    let parsed = grove_core::theme_file::parse_paste(buffer)?;
    let base = theme::by_name(DEFAULT_DARK_THEME).unwrap_or_else(|| theme::BUILTINS[0].clone());
    let mut draft = base;
    grove_core::theme_file::apply_pasted_colors(&mut draft, &parsed);
    let name = buffer
        .lines()
        .find_map(|l| l.strip_prefix("name:").map(|v| v.trim().to_string()))
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "my theme".to_string());
    draft.name = std::borrow::Cow::Owned(name.clone());
    let mut themes = theme::all_custom_themes();
    match themes.iter_mut().find(|t| t.name == name) {
        Some(existing) => *existing = draft,
        None => themes.push(draft),
    }
    persist_custom(&themes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lum(c: Hsla) -> f32 {
        let c: Rgba = c.into();
        0.2126 * c.r + 0.7152 * c.g + 0.0722 * c.b
    }

    #[test]
    fn mix_endpoints_and_midpoint() {
        let a = Rgba {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        };
        let b = Rgba {
            r: 1.0,
            g: 0.5,
            b: 0.25,
            a: 1.0,
        };
        assert_eq!(mix(a, b, 0.0).r, 0.0);
        assert_eq!(mix(a, b, 1.0).r, 1.0);
        let m = mix(a, b, 0.5);
        assert!((m.r - 0.5).abs() < 1e-6);
        assert!((m.g - 0.25).abs() < 1e-6);
        assert!((m.b - 0.125).abs() < 1e-6);
    }

    #[test]
    fn mix_clamps_out_of_range_t() {
        let a = ic(theme::Color::Rgb(0, 0, 0));
        let b = ic(theme::Color::Rgb(255, 255, 255));
        assert_eq!(mix(a, b, -1.0).r, 0.0);
        assert_eq!(mix(a, b, 2.0).r, 1.0);
    }

    #[test]
    fn ic_round_trips_known_rgb() {
        for (r, g, b) in [(0, 0, 0), (255, 255, 255), (0x1a, 0x1b, 0x26)] {
            let c = ic(theme::Color::Rgb(r, g, b));
            assert_eq!((c.r * 255.0).round() as u8, r);
            assert_eq!((c.g * 255.0).round() as u8, g);
            assert_eq!((c.b * 255.0).round() as u8, b);
            assert_eq!(c.a, 1.0);
        }
    }

    /// The default active theme is TokyoNight dark (`crates/grove-core/src/theme.rs:695`).
    #[test]
    fn default_theme_is_tokyonight_dark() {
        theme::with_current(|t| {
            assert_eq!(t.name, "tokyonight");
            assert_eq!(t.bg, theme::Color::Rgb(0x1a, 0x1b, 0x26));
            assert!(is_dark_of(t));
        });
        let bg: Rgba = BG().into();
        assert_eq!((bg.r * 255.0).round() as u8, 0x1a);
        assert_eq!((bg.g * 255.0).round() as u8, 0x1b);
        assert_eq!((bg.b * 255.0).round() as u8, 0x26);
    }

    #[test]
    fn amber_sits_between_yellow_and_red() {
        let (y, r, a) = theme::with_current(|t| (ic(t.yellow), ic(t.red), amber_rgba()));
        for (yc, rc, ac) in [(y.r, r.r, a.r), (y.g, r.g, a.g), (y.b, r.b, a.b)] {
            let (lo, hi) = if yc <= rc { (yc, rc) } else { (rc, yc) };
            assert!(
                ac >= lo - 1e-6 && ac <= hi + 1e-6,
                "{ac} not in [{lo},{hi}]"
            );
        }
        // 25% toward red, so it stays nearer yellow than red.
        assert!((a.r - y.r).abs() <= (a.r - r.r).abs() + 1e-6);
    }

    /// The chrome stack depends on this ordering: strip is the darkest
    /// surface, then the rail, then the body.
    #[test]
    fn chrome_surfaces_get_progressively_darker() {
        assert!(lum(BG_STRIP()) < lum(BG_RAIL()));
        assert!(lum(BG_RAIL()) < lum(BG()));
    }

    #[test]
    fn follow_system_resolution_picks_by_appearance() {
        let s = ThemeState::new(true, "tokyonight-storm".into(), "tokyonight-day".into());
        assert_eq!(
            s.resolve_system_theme_name(WindowAppearance::Dark),
            "tokyonight-storm"
        );
        assert_eq!(
            s.resolve_system_theme_name(WindowAppearance::VibrantDark),
            "tokyonight-storm"
        );
        assert_eq!(
            s.resolve_system_theme_name(WindowAppearance::Light),
            "tokyonight-day"
        );
        assert_eq!(
            s.resolve_system_theme_name(WindowAppearance::VibrantLight),
            "tokyonight-day"
        );
    }
}
