//! Font registration, the grid's cell metrics, and the startup metric assertion that is this phase's exit gate.

use gpui::{px, AssetSource as _, SharedString, TextRun};

use crate::assets::Assets;

/// Must stay aligned with the bundled BlexMono font at 12.5pt or the cursor drifts across long rows (`src/gui/metrics.rs:24-32`).
pub const CELL_W: f32 = 7.5; // measured advance at FONT_SIZE; == FONT_SIZE * 0.6
/// A GROVE constant, NOT a font metric — never use `window.line_height()` or `window.rem_size()` for the grid.
pub const CELL_H: f32 = 17.0;
pub const FONT_SIZE: f32 = 12.5;
/// `fc-scan` and gpui's `all_font_names()` agree on this spelling.
pub const MONO_FAMILY: &str = "BlexMono Nerd Font Mono";
pub const UI_FAMILY: &str = "IBM Plex Sans";

const FONT_FILES: [&str; 4] = [
    "fonts/BlexMonoNerdFontMono-Regular.ttf",
    "fonts/BlexMonoNerdFontMono-Bold.ttf",
    "fonts/IBMPlexSans-Regular.ttf",
    "fonts/IBMPlexSans-Bold.ttf",
];

/// A genuinely wrong font/size is off by >= 0.3px per cell; float noise from shaping is ~5e-7.
pub const CELL_W_EPSILON: f32 = 0.001;

pub fn metric_ok(measured: f32) -> bool {
    (measured - CELL_W).abs() < CELL_W_EPSILON
}

#[derive(Debug)]
pub enum MetricError {
    Registration(String),
    /// A required family is absent, so any measurement would come from a fallback font.
    MissingFamily {
        family: &'static str,
        available: Vec<String>,
    },
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

/// Must run before the window opens, using the same `Assets` handed to `with_assets`.
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

/// Mirrors the iced-side test at `src/gui/metrics.rs:388-392`, but at runtime since gpui's text system needs a live `App`.
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
    // Constructs a `WindowTextSystem` over the app's shared one, since no window exists yet but the shaping path must match.
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

/// A subtly-misaligned grid is worse than a refused start, and there is no UI yet to report into.
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
