//! ANSI → theme-token color resolution: the **only** place a `TermColor`
//! becomes a paintable color.
//!
//! Line-for-line port of `src/gui/pty.rs:374-421` (`vt_color_opt` / `ansi_idx`)
//! plus the inverse-swap rule at `src/gui/pty.rs:44-52`. Keeping the mapping in
//! one function is what lets a theme swap re-color live terminal content on the
//! next frame with no invalidation bookkeeping.

use gpui::Hsla;
use grove_core::theme::{self, Theme};
use grove_terminal::TermColor;

use crate::theme as c;

/// Resolve one terminal color against `theme`.
///
/// `None` means "the terminal's default": for a background that is *paint no
/// quad at all*, for a foreground it is the caller's cue to use the default fg
/// token (see [`resolve_pair`]).
pub fn resolve(color: TermColor, theme: &Theme) -> Option<Hsla> {
    match color {
        // `src/gui/pty.rs:376` — Default stays unresolved.
        TermColor::Default => None,
        TermColor::Ansi(i) => Some(ansi_idx(i, theme)),
        // `:378` — a 24-bit color bypasses the theme entirely.
        TermColor::Rgb(r, g, b) => Some(rgb8(r, g, b)),
    }
}

/// Resolve a cell's `(fg, bg)` pair, applying the inverse swap.
///
/// **This is the one and only inverse swap in the pipeline.**
/// `GroveTerm::snapshot()` does *not* pre-apply it — `Cell` carries `inverse:
/// bool` unswapped (`crates/grove-terminal/src/cell.rs:20`,
/// `term.rs:193`), because the golden harness applies the swap in its own
/// shared helper so the model and the vt100 oracle cannot drift. So the
/// painting layer owns it, exactly as `src/gui/pty.rs:44-52` does.
///
/// After the swap, a `None` fg becomes the theme's background and a `None` bg
/// becomes the theme's foreground: that "theme-default fill" is what makes an
/// inverse-video cell readable instead of transparent.
///
/// Returns `(fg, bg)` where the fg is always concrete and `bg == None` means
/// "emit no quad".
pub fn resolve_pair(
    fg: TermColor,
    bg: TermColor,
    inverse: bool,
    theme: &Theme,
) -> (Hsla, Option<Hsla>) {
    let mut fg = resolve(fg, theme);
    let mut bg = resolve(bg, theme);
    if inverse {
        std::mem::swap(&mut fg, &mut bg);
        if fg.is_none() {
            fg = Some(c::bg_of(theme).into());
        }
        if bg.is_none() {
            bg = Some(c::fg_of(theme).into());
        }
    }
    (fg.unwrap_or_else(|| c::fg_of(theme).into()), bg)
}

/// `src/gui/pty.rs:390-421`, index for index.
fn ansi_idx(i: u8, theme: &Theme) -> Hsla {
    match i {
        0 => c::bg_strip_of(theme).into(),
        1 | 9 => c::red_of(theme).into(),
        2 | 10 => c::green_of(theme).into(),
        3 | 11 => c::yellow_of(theme).into(),
        4 | 12 => c::blue_of(theme).into(),
        5 | 13 => c::magenta_of(theme).into(),
        6 | 14 => c::cyan_of(theme).into(),
        7 | 15 => c::fg_of(theme).into(),
        8 => c::fg_mute_of(theme).into(),
        16..=231 => {
            // 6×6×6 cube (`:401-415`).
            let n = i - 16;
            let r = n / 36;
            let g = (n % 36) / 6;
            let b = n % 6;
            rgb8(cube(r), cube(g), cube(b))
        }
        232..=255 => {
            // 24-step grayscale ramp (`:416-419`).
            let v = 8 + 10 * (i - 232);
            rgb8(v, v, v)
        }
    }
}

fn cube(x: u8) -> u8 {
    if x == 0 {
        0
    } else {
        55 + 40 * x
    }
}

/// 8-bit sRGB → `Hsla` through Plan 03's `ic()` conversion path, so a direct
/// color and a themed one land in the same color space (no HSL arithmetic).
fn rgb8(r: u8, g: u8, b: u8) -> Hsla {
    c::ic(theme::Color::Rgb(r, g, b)).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme() -> Theme {
        theme::with_current(Clone::clone)
    }

    /// Independent restatement of the table in the plan, so the test does not
    /// simply mirror the implementation's `match` arm order.
    fn expected(i: u8, t: &Theme) -> Hsla {
        match i {
            0 => c::bg_strip_of(t).into(),
            1 | 9 => c::red_of(t).into(),
            2 | 10 => c::green_of(t).into(),
            3 | 11 => c::yellow_of(t).into(),
            4 | 12 => c::blue_of(t).into(),
            5 | 13 => c::magenta_of(t).into(),
            6 | 14 => c::cyan_of(t).into(),
            7 | 15 => c::fg_of(t).into(),
            8 => c::fg_mute_of(t).into(),
            16..=231 => {
                let n = (i as u32) - 16;
                let v = |x: u32| -> u8 {
                    if x == 0 {
                        0
                    } else {
                        (55 + 40 * x) as u8
                    }
                };
                rgb8(v(n / 36), v((n % 36) / 6), v(n % 6))
            }
            232..=255 => {
                let v = 8 + 10 * (i - 232);
                rgb8(v, v, v)
            }
        }
    }

    #[test]
    fn every_ansi_index_matches_the_iced_table() {
        let t = theme();
        for i in 0..=255u8 {
            let got = resolve(TermColor::Ansi(i), &t);
            assert_eq!(got, Some(expected(i, &t)), "ansi index {i}");
        }
    }

    #[test]
    fn cube_and_gray_boundaries() {
        let t = theme();
        // 16 is the cube's black corner, 231 its white corner (`pty.rs:401`).
        assert_eq!(resolve(TermColor::Ansi(16), &t), Some(rgb8(0, 0, 0)));
        assert_eq!(resolve(TermColor::Ansi(231), &t), Some(rgb8(255, 255, 255)));
        // 232 is the darkest gray, 255 the lightest (`:416-419`).
        assert_eq!(resolve(TermColor::Ansi(232), &t), Some(rgb8(8, 8, 8)));
        assert_eq!(resolve(TermColor::Ansi(255), &t), Some(rgb8(238, 238, 238)));
    }

    #[test]
    fn bright_variants_fold_onto_their_base_token() {
        let t = theme();
        for base in 1..=7u8 {
            assert_eq!(
                resolve(TermColor::Ansi(base), &t),
                resolve(TermColor::Ansi(base + 8), &t),
                "bright {} must equal base {base}",
                base + 8
            );
        }
    }

    #[test]
    fn rgb_bypasses_the_theme_and_default_is_none() {
        let t = theme();
        assert_eq!(
            resolve(TermColor::Rgb(1, 2, 3), &t),
            Some(rgb8(1, 2, 3)),
            "a 24-bit color must not be remapped"
        );
        assert_eq!(resolve(TermColor::Default, &t), None);
    }

    #[test]
    fn plain_pair_uses_the_default_fg_token_and_paints_no_bg() {
        let t = theme();
        let (fg, bg) = resolve_pair(TermColor::Default, TermColor::Default, false, &t);
        assert_eq!(fg, c::fg_of(&t).into());
        assert_eq!(bg, None, "a default background must emit no quad");
    }

    #[test]
    fn inverse_swaps_exactly_once() {
        let t = theme();
        // The pipeline must swap once and only once: a red-on-default cell
        // rendered inverse is default-bg text on a red field, NOT red-on-default
        // again (which is what a second swap would give).
        let (fg, bg) = resolve_pair(TermColor::Ansi(1), TermColor::Default, true, &t);
        assert_eq!(
            fg,
            c::bg_of(&t).into(),
            "inverse fg = theme background fill"
        );
        assert_eq!(bg, Some(c::red_of(&t).into()));

        let plain = resolve_pair(TermColor::Ansi(1), TermColor::Default, false, &t);
        assert_ne!((fg, bg), plain, "inverse must not round-trip to normal");
    }

    #[test]
    fn inverse_of_a_default_pair_fills_with_theme_defaults() {
        let t = theme();
        // `pty.rs:46-51`: after the swap both sides are still None, so fg
        // becomes bg_of and bg becomes fg_of — the readable fill.
        let (fg, bg) = resolve_pair(TermColor::Default, TermColor::Default, true, &t);
        assert_eq!(fg, c::bg_of(&t).into());
        assert_eq!(bg, Some(c::fg_of(&t).into()));
    }

    #[test]
    fn inverse_of_a_fully_specified_pair_is_a_plain_swap() {
        let t = theme();
        let (fg, bg) = resolve_pair(TermColor::Ansi(2), TermColor::Ansi(4), true, &t);
        assert_eq!(fg, c::blue_of(&t).into());
        assert_eq!(bg, Some(c::green_of(&t).into()));
    }
}
