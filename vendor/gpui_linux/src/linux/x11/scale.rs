//! X11 startup scale selection, independent of the display connection.
//!
//! Desktop scale requests take precedence over an estimate from monitor size.
//! App zoom and GDK_DPI_SCALE (a text adjustment) are separate concerns.

pub(crate) const GPUI_X11_SCALE_FACTOR_ENV: &str = "GPUI_X11_SCALE_FACTOR";

#[derive(Debug, PartialEq)]
pub(crate) struct ScaleSelection {
    pub factor: f32,
    pub source: &'static str,
}

#[inline]
pub(crate) fn valid_scale_factor(scale: f32) -> bool {
    scale.is_sign_positive() && scale.is_normal()
}

/// Preserve the explicit override contract, including errors and `randr` mode.
/// The RandR query is lazy: it is unnecessary when the desktop supplies a scale.
pub(crate) fn select_scale(
    explicit: Option<&str>,
    gdk_scale: Option<&str>,
    xft_dpi: Option<&str>,
    randr: impl FnOnce() -> Option<f32>,
    mut warn: impl FnMut(String),
) -> Result<ScaleSelection, String> {
    if let Some(value) = explicit.filter(|value| !value.is_empty()) {
        if value.eq_ignore_ascii_case("randr") {
            return Ok(randr_scale(
                randr(),
                "GPUI_X11_SCALE_FACTOR=randr",
                &mut warn,
            ));
        }
        return match value.parse::<f32>() {
            Ok(factor) if valid_scale_factor(factor) => Ok(ScaleSelection {
                factor,
                source: GPUI_X11_SCALE_FACTOR_ENV,
            }),
            Ok(_) => Err(format!(
                "`{GPUI_X11_SCALE_FACTOR_ENV}` must be a positive normal number or `randr`. Got `{value}`"
            )),
            Err(_) => Err(format!(
                "`{GPUI_X11_SCALE_FACTOR_ENV}` must be a positive number or `randr`. Got `{value}`"
            )),
        };
    }

    for (source, value, divisor) in [("GDK_SCALE", gdk_scale, 1.0), ("Xft.dpi", xft_dpi, 96.0)] {
        if let Some(value) = value {
            if let Ok(parsed) = value.parse::<f32>() {
                let factor = parsed / divisor;
                if valid_scale_factor(factor) {
                    return Ok(ScaleSelection { factor, source });
                }
            }
            warn(format!("Ignoring invalid {source} scale value `{value}`"));
        }
    }

    Ok(randr_scale(randr(), "RandR", &mut warn))
}

fn randr_scale(
    factor: Option<f32>,
    source: &'static str,
    warn: &mut impl FnMut(String),
) -> ScaleSelection {
    if let Some(factor) = factor {
        if valid_scale_factor(factor) {
            return ScaleSelection { factor, source };
        }
        warn(format!("Ignoring invalid RandR scale factor `{factor}`"));
    } else if source != "RandR" {
        warn("Failed to calculate RandR scale factor, falling back to default".into());
    }
    ScaleSelection {
        factor: 1.0,
        source: "default",
    }
}
