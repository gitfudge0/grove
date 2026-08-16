//! GUI color tokens derived live from the active [`grove_core::theme`]. Missing surfaces are synthesized by blending the base colors.
//! Blending happens in sRGB (`gpui::Rgba`), converting to `Hsla` only at the end — blending in HSL space would shift hues across every theme.

#![allow(non_snake_case)]
// File-level: this module is the colour-role vocabulary — a role is declared because the palette defines it, not because a widget draws it today.
#![allow(dead_code)]

use gpui::{BorrowAppContext as _, Hsla, Rgba, WindowAppearance};
use grove_core::theme;

/// Grove's defaults when the store names no theme.
pub const DEFAULT_DARK_THEME: &str = "tokyonight-storm";
pub const DEFAULT_LIGHT_THEME: &str = "tokyonight-day";

const BLACK: Rgba = Rgba {
    r: 0.0,
    g: 0.0,
    b: 0.0,
    a: 1.0,
};

/// Public so a theme editor can render draft-theme swatches without going through `theme::current()`.
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

/// Component-wise lerp on sRGB floats — never in HSL space.
fn mix(a: Rgba, b: Rgba, t: f32) -> Rgba {
    let t = t.clamp(0.0, 1.0);
    Rgba {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: a.a,
    }
}

fn alpha_rgba(c: Rgba, a: f32) -> Rgba {
    Rgba { a, ..c }
}

/// The sanctioned way to tint a token — prefer over an inline `Hsla { a: .., ..c::TOKEN() }`, which hides tint sites from a grep.
pub fn alpha(c: Hsla, a: f32) -> Hsla {
    Hsla { a, ..c }
}

fn base_bg() -> Rgba {
    theme::with_current(|t| ic(t.bg))
}
fn base_fg() -> Rgba {
    theme::with_current(|t| ic(t.fg))
}

pub fn BG() -> Hsla {
    base_bg().into()
}

pub fn BG_RAIL() -> Hsla {
    theme::with_current(bg_rail_of).into()
}

pub fn BG_STRIP() -> Hsla {
    theme::with_current(bg_strip_of).into()
}

pub fn BG_HOVER() -> Hsla {
    theme::with_current(bg_hover_of).into()
}

pub fn BG_HL() -> Hsla {
    theme::with_current(|t| ic(t.bg_highlight)).into()
}

pub fn BORDER() -> Hsla {
    theme::with_current(border_of).into()
}
pub fn BORDER_SOFT() -> Hsla {
    theme::with_current(|t| mix(ic(t.bg), ic(t.fg), 0.07)).into()
}

/// Dark themes dim toward black; light themes dim toward the foreground so the wash stays visible on near-white backgrounds.
pub fn SCRIM() -> Hsla {
    theme::with_current(|t| {
        let dark = is_dark_of(t);
        let toward = if dark { BLACK } else { ic(t.fg) };
        let alpha = if dark { 0.62 } else { 0.7 };
        alpha_rgba(mix(ic(t.bg), toward, 0.9), alpha)
    })
    .into()
}

/// Black at an alpha, not a theme-color blend — a shadow tinted with the palette would read as a glow.
pub fn PANEL_SHADOW() -> Hsla {
    theme::with_current(|t| alpha_rgba(BLACK, if is_dark_of(t) { 0.35 } else { 0.18 })).into()
}

/// Views pick geometry (shadow offset/blur) by this; colour forks stay inside this module.
pub fn is_dark() -> bool {
    theme::with_current(is_dark_of)
}

pub fn FG() -> Hsla {
    base_fg().into()
}
pub fn FG_DIM() -> Hsla {
    theme::with_current(|t| ic(t.fg_dark)).into()
}
pub fn FG_MUTE() -> Hsla {
    theme::with_current(|t| ic(t.comment)).into()
}

pub fn BLUE() -> Hsla {
    theme::with_current(|t| ic(t.blue)).into()
}
pub fn CYAN() -> Hsla {
    theme::with_current(|t| ic(t.cyan)).into()
}
pub fn MAGENTA() -> Hsla {
    theme::with_current(|t| ic(t.magenta)).into()
}
/// Pushes the one gpui-component colour Grove actually renders into that crate's
/// own global theme.
///
/// `Input` draws its placeholder in `cx.theme().muted_foreground` — gpui-component's
/// palette, not Grove's — while a real value inherits `text_style.color` (`FG()`,
/// set on `panel_surface`). Left unsynced, a placeholder would sit at a fixed
/// third-party grey that ignores all 32 bundled themes, and the
/// placeholder-vs-value distinction §14 relies on would be whatever that grey
/// happened to look like. Called at startup and on every theme change.
pub fn sync_component_theme(cx: &mut gpui::App) {
    gpui_component::Theme::global_mut(cx).muted_foreground = FG_MUTE();
}

pub fn GREEN() -> Hsla {
    theme::with_current(|t| ic(t.green)).into()
}
/// The "needs input" accent — warmer than YELLOW so it reads as a call to action next to green/working.
pub fn AMBER() -> Hsla {
    amber_rgba().into()
}
fn amber_rgba() -> Rgba {
    theme::with_current(|t| mix(ic(t.yellow), ic(t.red), 0.25))
}
/// Fill behind a row whose session is waiting on you; shared by the sidebar and launcher waiting rows, hence a token not a module constant.
pub fn AMBER_ROW_TINT() -> Hsla {
    alpha(AMBER(), 0.12)
}
pub fn YELLOW() -> Hsla {
    theme::with_current(|t| ic(t.yellow)).into()
}
pub fn RED() -> Hsla {
    theme::with_current(|t| ic(t.red)).into()
}

/// Active fill for a danger-flavored segmented control (e.g. "skip permissions"), distinct from the neutral `BG_HL()`.
pub fn RED_WASH() -> Hsla {
    theme::with_current(|t| mix(ic(t.red), ic(t.bg), 0.84)).into()
}

pub fn DIFF_ADD_BG() -> Hsla {
    alpha(GREEN(), 0.12)
}
pub fn DIFF_DEL_BG() -> Hsla {
    alpha(RED(), 0.12)
}

/// [`DIFF_ADD_BG`] at 26%, for word-level intraline emphasis.
pub fn DIFF_ADD_BG_STRONG() -> Hsla {
    alpha(GREEN(), 0.26)
}
pub fn DIFF_DEL_BG_STRONG() -> Hsla {
    alpha(RED(), 0.26)
}

// Seven semantic scopes syntect spans are projected onto — never syntect's own theme colours.
pub fn CODE_KEYWORD() -> Hsla {
    MAGENTA()
}
pub fn CODE_STRING() -> Hsla {
    GREEN()
}
pub fn CODE_NUMBER() -> Hsla {
    YELLOW()
}
pub fn CODE_COMMENT() -> Hsla {
    FG_MUTE()
}
pub fn CODE_TYPE() -> Hsla {
    CYAN()
}
pub fn CODE_FUNC() -> Hsla {
    BLUE()
}
pub fn CODE_PUNCT() -> Hsla {
    FG_DIM()
}

fn cyan_rgba() -> Rgba {
    theme::with_current(|t| ic(t.cyan))
}

pub fn SEL_TINT_STRONG() -> Hsla {
    alpha_rgba(cyan_rgba(), 0.22).into()
}
pub fn SEL_TINT_SOFT() -> Hsla {
    alpha_rgba(cyan_rgba(), 0.10).into()
}
pub fn SEL_RING() -> Hsla {
    alpha_rgba(cyan_rgba(), 0.5).into()
}

// Renders PTY content under a per-project theme override, decoupled from the global `theme::current()`. Return Rgba (not Hsla) since they're blend inputs to each other.
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

pub fn bg_rail_of(t: &theme::Theme) -> Rgba {
    let d = if is_dark_of(t) { 0.18 } else { 0.04 };
    mix(ic(t.bg), BLACK, d)
}

/// Used for ANSI color 0 inside PTY content rendered under a per-project override theme.
pub fn bg_strip_of(t: &theme::Theme) -> Rgba {
    let bg = ic(t.bg);
    if is_dark_of(t) {
        mix(bg, BLACK, 0.32)
    } else {
        mix(bg, BLACK, 0.08)
    }
}

/// Reserved, no consumer yet — exists so a per-project theme can tint selected/highlighted PTY regions later; not dead code. `bg_hover_of` does read it.
pub fn bg_hl_of(t: &theme::Theme) -> Rgba {
    ic(t.bg_highlight)
}
pub fn bg_hover_of(t: &theme::Theme) -> Rgba {
    mix(bg_of(t), bg_hl_of(t), 0.55)
}
pub fn border_of(t: &theme::Theme) -> Rgba {
    mix(bg_of(t), fg_of(t), 0.16)
}
/// Reserved, no consumer yet — for a future PTY selection outline in the project's pinned theme; chrome selection uses `SEL_RING()`.
pub fn sel_ring_of(t: &theme::Theme) -> Rgba {
    alpha_rgba(cyan_of(t), 0.5)
}

/// Resolution policy only — the colors themselves live in `grove_core::theme::ACTIVE`.
pub struct ThemeState {
    pub follow_system: bool,
    pub dark_name: String,
    pub light_name: String,
    pub system_mode: WindowAppearance,
    /// Bumped on every change; the terminal element uses it as a repaint/cache key.
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

    /// Anything that isn't explicitly light resolves dark.
    pub fn resolve_system_theme_name(&self, mode: WindowAppearance) -> &str {
        match mode {
            WindowAppearance::Light | WindowAppearance::VibrantLight => &self.light_name,
            WindowAppearance::Dark | WindowAppearance::VibrantDark => &self.dark_name,
        }
    }

    /// Unknown names are ignored by grove-core, so the previous theme stays active.
    pub fn set_by_name(cx: &mut gpui::App, name: &str) {
        if grove_core::theme::set_by_name(name) {
            cx.update_global::<Self, _>(|this, _| this.generation += 1);
            sync_component_theme(cx);
            cx.refresh_windows();
        }
    }

    /// No-op unless `follow_system` is set.
    pub fn apply_system_theme(cx: &mut gpui::App) {
        let name = cx.global::<Self>();
        if !name.follow_system {
            return;
        }
        let name = name.resolve_system_theme_name(name.system_mode).to_string();
        Self::set_by_name(cx, &name);
    }

    /// Called once with the first frame's `appearance()` — seeding it there is why follow-system looks correct immediately.
    pub fn set_system_mode(cx: &mut gpui::App, mode: WindowAppearance) {
        cx.update_global::<Self, _>(|this, _| this.system_mode = mode);
        Self::apply_system_theme(cx);
    }
}

// Every mutation goes through `grove_core::theme_file::save` then `load_custom`, so `theme::CUSTOM` and `themes.json` can never disagree.

/// The paste-first editor's starting buffer.
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

/// The buffer is `to_named_lines`' output (or a pasted equivalent), round-tripped through `theme_file::parse_paste` onto a draft derived from the default theme.
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

    fn lum_rgba(c: Rgba) -> f32 {
        0.2126 * c.r + 0.7152 * c.g + 0.0722 * c.b
    }

    fn lum(c: Hsla) -> f32 {
        lum_rgba(c.into())
    }

    /// Serializes tests that mutate the global active theme, so a swap can't race a concurrent default-reader.
    static ACTIVE_THEME_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Restores the pre-test active theme, even on panic.
    struct ActiveThemeGuard(theme::Theme);
    impl ActiveThemeGuard {
        fn capture() -> Self {
            Self(theme::current())
        }
    }
    impl Drop for ActiveThemeGuard {
        fn drop(&mut self) {
            theme::set(self.0.clone());
        }
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
        let _lock = ACTIVE_THEME_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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

    /// AMBER is `mix(yellow, red, 0.25)`, checked against every bundled theme.
    #[test]
    fn amber_sits_between_yellow_and_red() {
        // Reads the live accessors below, so it must serialize against the tests
        // that swap the active theme (pre-existing race; the lock is the file's
        // established guard).
        let _lock = ACTIVE_THEME_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert!(theme::BUILTINS.len() > 30, "expected the full bundled set");
        for t in theme::BUILTINS {
            let (y, r) = (yellow_of(t), red_of(t));
            let a = mix(y, r, 0.25);
            for (ch, yc, rc, ac) in [
                ('r', y.r, r.r, a.r),
                ('g', y.g, r.g, a.g),
                ('b', y.b, r.b, a.b),
            ] {
                let (lo, hi) = if yc <= rc { (yc, rc) } else { (rc, yc) };
                assert!(
                    ac >= lo - 1e-6 && ac <= hi + 1e-6,
                    "theme '{}': amber {ch} {ac} not in [{lo},{hi}]",
                    t.name
                );
                assert!(
                    (ac - yc).abs() <= (ac - rc).abs() + 1e-6,
                    "theme '{}': amber {ch} {ac} is nearer red {rc} than yellow {yc}",
                    t.name
                );
                let expected = yc + (rc - yc) * 0.25;
                assert!(
                    (ac - expected).abs() < 1e-6,
                    "theme '{}': amber {ch} {ac} != mix(yellow,red,0.25) = {expected}",
                    t.name
                );
            }
        }
        let (y, r, a) = theme::with_current(|t| (ic(t.yellow), ic(t.red), amber_rgba()));
        let want = mix(y, r, 0.25);
        assert!((a.r - want.r).abs() < 1e-6 && (a.g - want.g).abs() < 1e-6);
    }

    /// Chrome stack: strip is darkest, then rail, then body — checked on every bundled theme, including light.
    #[test]
    fn chrome_surfaces_get_progressively_darker() {
        // Reads the live accessors below, so it must serialize against the tests
        // that swap the active theme (pre-existing race; the lock is the file's
        // established guard).
        let _lock = ACTIVE_THEME_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert!(theme::BUILTINS.len() > 30, "expected the full bundled set");
        for t in theme::BUILTINS {
            let (strip, rail, bg) = (
                lum_rgba(bg_strip_of(t)),
                lum_rgba(bg_rail_of(t)),
                lum_rgba(bg_of(t)),
            );
            assert!(
                strip < rail,
                "theme '{}': BG_STRIP luminance {strip} is not darker than BG_RAIL {rail}",
                t.name
            );
            assert!(
                rail < bg,
                "theme '{}': BG_RAIL luminance {rail} is not darker than BG {bg}",
                t.name
            );
        }
        assert!(lum(BG_STRIP()) < lum(BG_RAIL()));
        assert!(lum(BG_RAIL()) < lum(BG()));
    }

    /// PANEL_SHADOW's alpha and geometry (Y/BLUR) must both be strictly larger on dark, or a light panel inherits a dark-theme shadow.
    #[test]
    fn panel_shadow_is_heavier_in_dark_themes_than_light() {
        use crate::views::tokens::{
            PANEL_SHADOW_BLUR, PANEL_SHADOW_BLUR_LIGHT, PANEL_SHADOW_Y, PANEL_SHADOW_Y_LIGHT,
        };
        assert!(theme::BUILTINS.len() > 30, "expected the full bundled set");
        // black_box defeats constant folding, so clippy's assertions_on_constants doesn't flag this intentional comparison.
        let (y, y_light) = (
            std::hint::black_box(PANEL_SHADOW_Y),
            std::hint::black_box(PANEL_SHADOW_Y_LIGHT),
        );
        assert!(
            y > y_light,
            "dark-theme shadow offset must exceed the light-theme offset"
        );
        let (blur, blur_light) = (
            std::hint::black_box(PANEL_SHADOW_BLUR),
            std::hint::black_box(PANEL_SHADOW_BLUR_LIGHT),
        );
        assert!(
            blur > blur_light,
            "dark-theme shadow blur must exceed the light-theme blur"
        );
        let dark_alpha: f32 = alpha_rgba(BLACK, 0.35).a;
        let light_alpha: f32 = alpha_rgba(BLACK, 0.18).a;
        assert!(
            dark_alpha > light_alpha,
            "dark-theme shadow alpha {dark_alpha} must exceed light-theme alpha {light_alpha}"
        );
        let _lock = ACTIVE_THEME_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _guard = ActiveThemeGuard::capture();

        let (mut saw_dark, mut saw_light) = (0u32, 0u32);
        for t in theme::BUILTINS {
            let want = if is_dark_of(t) { 0.35 } else { 0.18 };
            let expected: Rgba = alpha_rgba(BLACK, want);

            theme::set(t.clone());
            let live: Rgba = PANEL_SHADOW().into();

            assert!(
                (live.a - want).abs() < 1e-6,
                "theme '{}': PANEL_SHADOW alpha {} != expected {want}",
                t.name,
                live.a
            );
            assert!(
                (live.a - expected.a).abs() < 1e-6,
                "theme '{}': PANEL_SHADOW alpha {} disagrees with independent derivation {}",
                t.name,
                live.a,
                expected.a
            );
            if !is_dark_of(t) {
                assert!(
                    (live.a - 0.35).abs() > 1e-6,
                    "theme '{}': light theme still wears the dark-theme literal 0.35",
                    t.name
                );
                saw_light += 1;
            } else {
                saw_dark += 1;
            }
        }
        assert!(saw_dark > 0, "no dark theme was exercised");
        assert!(saw_light > 0, "no light theme was exercised");
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
