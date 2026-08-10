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
// File-level by design: this module is the colour-role vocabulary. A role is
// declared because the palette defines it, not because a widget happens to draw
// it today, so unused roles are expected rather than dead.
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

fn alpha_rgba(c: Rgba, a: f32) -> Rgba {
    Rgba { a, ..c }
}

/// Overrides a token's alpha and nothing else — hue, saturation and lightness
/// are untouched, so the result still tracks a theme swap.
///
/// This is the **sanctioned** way to tint a tier-2 token at a call site. Prefer
/// it to writing `Hsla { a: .., ..c::TOKEN() }` inline: the struct-update form
/// is the same operation spelled ad-hoc, and it hides tint sites from a grep.
/// If two or more call sites want the *same* alpha on the same token, promote
/// it to a named token here instead (§14.3).
pub fn alpha(c: Hsla, a: f32) -> Hsla {
    Hsla { a, ..c }
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
    theme::with_current(bg_rail_of).into()
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
///
/// Shared by every modal (`scrim`/`scrim_top_drop`, `views/components.rs`),
/// so this is a global bump rather than a modal-local override: every modal
/// benefits from the background actually receding instead of just the one
/// under review. `0.16` read as barely-there — PTY text stayed fully legible
/// through it — so both tones sit inside the requested α0.55-0.70 band, with
/// light themes a touch stronger since a bright page reads through a wash
/// more than a dark one does at the same alpha.
pub fn SCRIM() -> Hsla {
    theme::with_current(|t| {
        let dark = is_dark_of(t);
        let toward = if dark { BLACK } else { ic(t.fg) };
        let alpha = if dark { 0.62 } else { 0.7 };
        alpha_rgba(mix(ic(t.bg), toward, 0.9), alpha)
    })
    .into()
}

/// The drop shadow every floating panel casts (plan.md §3's last bullet). Was
/// a hard-coded `rgba(0,0,0,.35)` inside `modal_panel`, which is why a light
/// theme's panel wore a dark theme's shadow: a bright page needs a lighter,
/// tighter shadow or the panel reads as cut out of the page rather than lifted
/// off it. The *geometry* that goes with each weight is
/// [`PANEL_SHADOW_Y`](crate::views::tokens::PANEL_SHADOW_Y) and friends,
/// selected by [`is_dark`].
///
/// Black at an alpha, not a blend of the theme's colors: a shadow is absence of
/// light, so tinting it with the palette would make it read as a glow.
pub fn PANEL_SHADOW() -> Hsla {
    theme::with_current(|t| alpha_rgba(BLACK, if is_dark_of(t) { 0.35 } else { 0.18 })).into()
}

/// Whether the active theme is a dark one — the view layer's read-only handle
/// on the same question [`is_dark_of`] answers for a borrowed theme. Views pick
/// *geometry* by it (the panel shadow's offset and blur); colour forks stay
/// inside this module.
pub fn is_dark() -> bool {
    theme::with_current(is_dark_of)
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
/// The keyboard-focus ring: MAGENTA held back to a tint, drawn *outside* a
/// focused control's own 1px magenta border (plan.md §1, variant C1c). One
/// token, two weights: a dark theme reads a 25% magenta wash on a dark surface
/// clearly, while the same alpha over a near-white light surface all but
/// disappears, so light themes get a stronger tint rather than a second token.
///
/// Derived with [`alpha`] rather than a hand-written `Hsla` literal so the ring
/// tracks a theme swap exactly as MAGENTA does (§14.3).
pub fn FOCUS_RING() -> Hsla {
    let a = theme::with_current(|t| if is_dark_of(t) { 0.25 } else { 0.35 });
    alpha(MAGENTA(), a)
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
/// AMBER at α 0.12 — the fill behind a row whose session is waiting on you.
/// Faint enough that the row's text keeps its contrast, strong enough to pick
/// the row out of a list at a glance; the *glyph* is what names the state
/// (§2.3), this only locates it. Shared by the sidebar's waiting row
/// (`src/views/rows.rs`) and the launcher's waiting row
/// (`src/views/modals/launcher.rs`), which is why it is a token and not a
/// module constant (§14.3).
pub fn AMBER_ROW_TINT() -> Hsla {
    alpha(AMBER(), 0.12)
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
// A selected row is marked with a flat cyan tint plus a cyan ring, in two
// weights. Derived from the theme's cyan so the treatment tracks theme swaps.

fn cyan_rgba() -> Rgba {
    theme::with_current(|t| ic(t.cyan))
}

/// Fill for a row in edit/rename mode — one weight up from plain selection.
pub fn SEL_TINT_STRONG() -> Hsla {
    alpha_rgba(cyan_rgba(), 0.22).into()
}
/// Fill for an ordinary selected row (palette rows, modal lists).
pub fn SEL_TINT_SOFT() -> Hsla {
    alpha_rgba(cyan_rgba(), 0.10).into()
}
/// Ring outlining a selected row at either weight.
pub fn SEL_RING() -> Hsla {
    alpha_rgba(cyan_rgba(), 0.5).into()
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

/// Themed variant of `BG_RAIL`. Same ratios as `BG_RAIL`, which delegates here.
pub fn bg_rail_of(t: &theme::Theme) -> Rgba {
    let d = if is_dark_of(t) { 0.18 } else { 0.04 };
    mix(ic(t.bg), BLACK, d)
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
///
/// **Reserved, no consumer yet.** Part of the PTY-content contract (§4.4): it
/// exists so a per-project theme can tint selected/highlighted PTY regions
/// without reaching for the global accessor. Nothing paints with it today —
/// it is a contract ahead of its consumer (§15.4), not dead code to delete.
/// `bg_hover_of` below does read it.
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
///
/// **Reserved, no consumer yet.** Same status as `bg_hl_of`: it is here so a
/// future PTY selection outline can be drawn in the project's pinned theme
/// rather than the global one (§4.4). Chrome selection uses `SEL_RING()`.
pub fn sel_ring_of(t: &theme::Theme) -> Rgba {
    alpha_rgba(cyan_of(t), 0.5)
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

    fn lum_rgba(c: Rgba) -> f32 {
        0.2126 * c.r + 0.7152 * c.g + 0.0722 * c.b
    }

    fn lum(c: Hsla) -> f32 {
        lum_rgba(c.into())
    }

    /// Serializes tests that mutate the global active theme
    /// (`grove_core::theme::set`) — nothing else in this crate touches it,
    /// but a test that temporarily swaps it must not race a concurrently
    /// running test that reads the default (e.g.
    /// `default_theme_is_tokyonight_dark`).
    static ACTIVE_THEME_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Restores whatever theme was active before the test ran, even on panic.
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

    /// AMBER is `mix(yellow, red, 0.25)` — inside the yellow→red interval on
    /// every channel, and nearer yellow on every channel. Checked against every
    /// bundled theme, since the ratio must hold whatever yellow and red are.
    #[test]
    fn amber_sits_between_yellow_and_red() {
        // Guard against a vacuous pass if BUILTINS ever ships empty.
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
                // 25% toward red, so it stays nearer yellow than red.
                assert!(
                    (ac - yc).abs() <= (ac - rc).abs() + 1e-6,
                    "theme '{}': amber {ch} {ac} is nearer red {rc} than yellow {yc}",
                    t.name
                );
                // The derivation itself, component-wise.
                let expected = yc + (rc - yc) * 0.25;
                assert!(
                    (ac - expected).abs() < 1e-6,
                    "theme '{}': amber {ch} {ac} != mix(yellow,red,0.25) = {expected}",
                    t.name
                );
            }
        }
        // The live accessor agrees with the parameterized derivation.
        let (y, r, a) = theme::with_current(|t| (ic(t.yellow), ic(t.red), amber_rgba()));
        let want = mix(y, r, 0.25);
        assert!((a.r - want.r).abs() < 1e-6 && (a.g - want.g).abs() < 1e-6);
    }

    /// The chrome stack depends on this ordering: strip is the darkest
    /// surface, then the rail, then the body. DESIGN.md §4.2 claims it holds
    /// on light themes too, so every bundled theme is checked.
    #[test]
    fn chrome_surfaces_get_progressively_darker() {
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
        // The live accessors agree with the parameterized variants.
        assert!(lum(BG_STRIP()) < lum(BG_RAIL()));
        assert!(lum(BG_RAIL()) < lum(BG()));
    }

    /// FOCUS_RING is MAGENTA with only its alpha overridden (plan.md §1):
    /// hue/saturation/lightness must be bit-identical to MAGENTA's, and alpha
    /// must be exactly 0.25 on a dark theme / 0.35 on a light one — the two
    /// weights `FOCUS_RING`'s doc comment names. Checked against every
    /// bundled theme, following the same template as
    /// `amber_sits_between_yellow_and_red`.
    #[test]
    fn focus_ring_derives_from_magenta_at_the_theme_relative_alpha() {
        assert!(theme::BUILTINS.len() > 30, "expected the full bundled set");
        let _lock = ACTIVE_THEME_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _guard = ActiveThemeGuard::capture();

        let (mut saw_dark, mut saw_light) = (0u32, 0u32);
        for t in theme::BUILTINS {
            // Independent, per-theme expectation — built from the theme's
            // raw magenta, not from anything the live accessor touches.
            let m: Hsla = magenta_of(t).into();
            let want_a = if is_dark_of(t) { 0.25 } else { 0.35 };
            let expected = alpha(m, want_a);

            // The live accessor, exercised for real by making this theme
            // the active one — not re-derived by hand a second time.
            theme::set(t.clone());
            let live = FOCUS_RING();

            assert_eq!(
                (live.h, live.s, live.l),
                (expected.h, expected.s, expected.l),
                "theme '{}': FOCUS_RING hue/sat/lightness drifted from MAGENTA's",
                t.name
            );
            assert!(
                (live.a - want_a).abs() < 1e-6,
                "theme '{}': FOCUS_RING alpha {} != expected {want_a} ({})",
                t.name,
                live.a,
                if is_dark_of(t) { "dark" } else { "light" }
            );
            if is_dark_of(t) {
                saw_dark += 1;
            } else {
                saw_light += 1;
            }
        }
        assert!(saw_dark > 0, "no dark theme was exercised");
        assert!(saw_light > 0, "no light theme was exercised");
    }

    /// PANEL_SHADOW is heavier on dark themes than light (plan.md §3's last
    /// bullet): both the colour's alpha and the accompanying
    /// `PANEL_SHADOW_Y`/`PANEL_SHADOW_BLUR` geometry must be strictly larger
    /// on the dark branch, in every bundled theme, or a light panel inherits
    /// a dark-theme shadow that reads as cut out of the page rather than
    /// lifted off it.
    #[test]
    fn panel_shadow_is_heavier_in_dark_themes_than_light() {
        use crate::views::tokens::{
            PANEL_SHADOW_BLUR, PANEL_SHADOW_BLUR_LIGHT, PANEL_SHADOW_Y, PANEL_SHADOW_Y_LIGHT,
        };
        assert!(theme::BUILTINS.len() > 30, "expected the full bundled set");
        // `black_box` defeats constant folding so clippy's
        // `assertions_on_constants` doesn't flag comparing two `const`
        // tokens whose relationship is exactly what this test exists to
        // pin — the values themselves are still read from the real tokens,
        // not duplicated as literals.
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
            // Independent, per-theme expectation.
            let want = if is_dark_of(t) { 0.35 } else { 0.18 };
            let expected: Rgba = alpha_rgba(BLACK, want);

            // The live accessor, exercised for real under this theme.
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
            // Never the pre-C2g hard-coded literal on a light theme.
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
