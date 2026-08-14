//! ANSI → theme-token color resolution: the only place a `TermColor` becomes
//! a paintable color, so a theme swap re-colors live content with no
//! invalidation bookkeeping.

use gpui::Hsla;
use grove_core::theme::{self, Theme};
use grove_terminal::TermColor;

use crate::theme as c;

/// `None` means "the terminal's default": paint-no-quad for a background, the caller's cue to use the default fg token for a foreground.
pub fn resolve(color: TermColor, theme: &Theme) -> Option<Hsla> {
    match color {
        TermColor::Default => None,
        TermColor::Ansi(i) => Some(ansi_idx(i, theme)),
        TermColor::Rgb(r, g, b) => Some(rgb8(r, g, b)),
    }
}

/// The one and only inverse swap in the pipeline — `Cell` carries `inverse` unswapped, so the painting layer owns it. Returns `(fg, bg)` where fg is always concrete and `bg == None` means "emit no quad".
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
            // 6x6x6 cube.
            let n = i - 16;
            let r = n / 36;
            let g = (n % 36) / 6;
            let b = n % 6;
            rgb8(cube(r), cube(g), cube(b))
        }
        232..=255 => {
            // 24-step grayscale ramp.
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

/// Through `ic()` so a direct color and a themed one land in the same color space.
fn rgb8(r: u8, g: u8, b: u8) -> Hsla {
    c::ic(theme::Color::Rgb(r, g, b)).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme() -> Theme {
        theme::with_current(Clone::clone)
    }

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
        assert_eq!(resolve(TermColor::Ansi(16), &t), Some(rgb8(0, 0, 0)));
        assert_eq!(resolve(TermColor::Ansi(231), &t), Some(rgb8(255, 255, 255)));
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
