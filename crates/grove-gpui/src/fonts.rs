//! Font registration, the grid's cell metrics, and the startup metric
//! assertion that is this phase's exit gate.

use gpui::{px, AssetSource as _, SharedString, TextRun};

use crate::assets::Assets;

/// Hardcoded cell metrics for BlexMono Nerd Font (IBM Plex Mono patched with
/// Nerd Font glyphs) at 12.5pt. It keeps Plex Mono's 600-unit advance on a
/// 1000-unit em, which gives a 7.5px cell at this size. The grid maps
/// (row, col) directly to pixels, so this must stay aligned with the bundled
/// font or the cursor drifts across long rows.
/// Copied from `src/gui/metrics.rs:24-32`.
pub const CELL_W: f32 = 7.5; // measured advance at FONT_SIZE; == FONT_SIZE * 0.6
/// A GROVE constant, NOT a font metric — never use `window.line_height()`
/// (26px) or `window.rem_size()` (16px) for the grid.
pub const CELL_H: f32 = 17.0;
pub const FONT_SIZE: f32 = 12.5;
/// `fc-scan` and gpui's `all_font_names()` agree on this spelling.
pub const MONO_FAMILY: &str = "BlexMono Nerd Font Mono";
pub const UI_FAMILY: &str = "IBM Plex Sans";

/// Every TTF bundled in `assets/fonts`, registered before the window opens.
const FONT_FILES: [&str; 4] = [
    "fonts/BlexMonoNerdFontMono-Regular.ttf",
    "fonts/BlexMonoNerdFontMono-Bold.ttf",
    "fonts/IBMPlexSans-Regular.ttf",
    "fonts/IBMPlexSans-Bold.ttf",
];

/// Epsilon is 0.001 px: the spike measured 7.5000005 at 12.5pt, so float noise
/// is ~5e-7 while a genuinely wrong font/size is off by >= 0.3px per cell.
pub const CELL_W_EPSILON: f32 = 0.001;

/// True when a measured mono advance matches the grid's `CELL_W`.
pub fn metric_ok(measured: f32) -> bool {
    (measured - CELL_W).abs() < CELL_W_EPSILON
}

/// What went wrong when the bundled mono font did not measure as expected.
#[derive(Debug)]
pub enum MetricError {
    /// `add_fonts` refused the bundled bytes.
    Registration(String),
    /// A required family is absent, so any measurement would come from a
    /// fallback font — the failure mode most likely to look plausible.
    MissingFamily {
        family: &'static str,
        available: Vec<String>,
    },
    /// The family is present but its advance is not `CELL_W`.
    WrongAdvance { expected: f32, measured: f32 },
}

impl std::fmt::Display for MetricError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Registration(e) => write!(f, "font registration failed: {e}"),
            Self::MissingFamily { family, available } => {
                write!(
                    f,
                    "font family {family:?} not registered; available: {available:?}"
                )
            }
            Self::WrongAdvance { expected, measured } => write!(
                f,
                "mono cell advance is {measured} px at {FONT_SIZE}pt, expected {expected} px"
            ),
        }
    }
}

/// Registers every bundled TTF with the text system. Must run **before** the
/// window opens, using the same `Assets` handed to `with_assets`.
pub fn register(cx: &mut gpui::App) -> Result<(), MetricError> {
    let mut bytes = Vec::with_capacity(FONT_FILES.len());
    for path in FONT_FILES {
        match Assets.load(path) {
            Ok(Some(b)) => bytes.push(b),
            Ok(None) => return Err(MetricError::Registration(format!("{path} not bundled"))),
            Err(e) => return Err(MetricError::Registration(format!("{path}: {e}"))),
        }
    }
    cx.text_system()
        .add_fonts(bytes)
        .map_err(|e| MetricError::Registration(e.to_string()))
}

/// Measures the em advance of the bundled mono font and fails loudly if it is
/// not exactly `CELL_W`. The grid maps (row, col) directly to pixels, so a
/// wrong advance silently drifts the cursor across long rows — mirrors the
/// iced-side test at `src/gui/metrics.rs:388-392`, but at RUNTIME, because
/// gpui's text system only exists inside a live `App`.
pub fn assert_cell_metrics(cx: &mut gpui::App) -> Result<f32, MetricError> {
    let available = cx.text_system().all_font_names();
    for family in [MONO_FAMILY, UI_FAMILY] {
        if !available.iter().any(|n| n == family) {
            return Err(MetricError::MissingFamily {
                family,
                available: available.clone(),
            });
        }
    }

    let text = SharedString::from("M");
    let run = TextRun {
        len: text.len(),
        font: gpui::font(MONO_FAMILY),
        color: gpui::hsla(0., 0., 1., 1.),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    // `shape_line` lives on `WindowTextSystem`, but the assertion must run
    // before any window exists — constructing one over the app's shared
    // `TextSystem` measures through exactly the same shaping path a window
    // would use.
    let shaper = gpui::WindowTextSystem::new(cx.text_system().clone());
    let measured = f32::from(shaper.shape_line(text, px(FONT_SIZE), &[run], None).width());

    if metric_ok(measured) {
        Ok(measured)
    } else {
        Err(MetricError::WrongAdvance {
            expected: CELL_W,
            measured,
        })
    }
}

/// Runs registration + the assertion, exiting the process on failure. A shell
/// that renders a subtly-misaligned grid is worse than one that refuses to
/// start, and there is no UI yet to report into.
pub fn register_and_assert_or_exit(cx: &mut gpui::App) -> f32 {
    let result = register(cx).and_then(|()| assert_cell_metrics(cx));
    match result {
        Ok(measured) => measured,
        Err(e) => {
            tracing::error!("grove-gpui: cell metric assertion failed: {e}");
            eprintln!("grove-gpui: cell metric assertion failed: {e}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_value_is_ok() {
        assert!(metric_ok(7.5));
    }

    #[test]
    fn spike_measured_value_is_ok() {
        assert!(metric_ok(7.500_000_5));
    }

    #[test]
    fn twelve_point_advance_is_rejected() {
        assert!(!metric_ok(7.2));
    }

    #[test]
    fn thirteen_point_advance_is_rejected() {
        assert!(!metric_ok(7.8));
    }

    #[test]
    fn missing_font_is_rejected() {
        assert!(!metric_ok(0.0));
    }
}
