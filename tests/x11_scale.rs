#[path = "../vendor/gpui_linux/src/linux/x11/scale.rs"]
mod scale;

use scale::ScaleSelection;
fn select_scale(
    explicit: Option<&str>,
    gdk: Option<&str>,
    xft: Option<&str>,
    randr: impl FnOnce() -> Option<f32>,
    warn: impl FnMut(String),
) -> Result<ScaleSelection, String> {
    scale::select_scale(explicit, gdk, xft, 1.0, randr, warn)
}

#[test]
fn xft_text_factor_is_removed_from_display_scale_only() {
    for (dpi, text, display) in [("144", 1.5, 1.), ("288", 1.5, 2.), ("120", 1., 1.25)] {
        let result = scale::select_scale(None, None, Some(dpi), text, no_randr, |_| {}).unwrap();
        assert_eq!(result.factor, display);
    }
    for (explicit, gdk) in [(Some("2"), None), (None, Some("2"))] {
        let result =
            scale::select_scale(explicit, gdk, Some("144"), 1.5, no_randr, |_| {}).unwrap();
        assert_eq!(result.factor, 2.);
    }
}

fn no_randr() -> Option<f32> {
    panic!("RandR must not be queried when a scale has already been selected")
}

#[test]
fn desktop_scale_overrides_physical_dpi_estimate() {
    // This desktop requests 2x, but its physical monitor DPI estimates 2.5x.
    let selected = select_scale(None, Some("2"), None, || Some(2.5), |_| {}).unwrap();
    assert_eq!(selected.factor, 2.0);
    assert_eq!(selected.source, "GDK_SCALE");
}

#[test]
fn explicit_scale_overrides_all_desktop_sources_without_querying_randr() {
    let selected = select_scale(Some("1.75"), Some("2"), Some("240"), no_randr, |_| {}).unwrap();
    assert_eq!(selected.factor, 1.75);
    assert_eq!(selected.source, "GPUI_X11_SCALE_FACTOR");
}

#[test]
fn randr_override_bypasses_desktop_sources_even_when_unavailable() {
    for value in ["randr", "RaNdR"] {
        let selected =
            select_scale(Some(value), Some("2"), Some("144"), || Some(2.5), |_| {}).unwrap();
        assert_eq!(selected.factor, 2.5);
        assert_eq!(selected.source, "GPUI_X11_SCALE_FACTOR=randr");
    }
    let mut warnings = Vec::new();
    let selected = select_scale(
        Some("randr"),
        Some("2"),
        Some("144"),
        || None,
        |warning| warnings.push(warning),
    )
    .unwrap();
    assert_eq!(selected.factor, 1.0);
    assert_eq!(selected.source, "default");
    assert_eq!(warnings.len(), 1);
}

#[test]
fn empty_override_uses_desktop_scale_and_gdk_precedes_xft() {
    let selected = select_scale(Some(""), Some("2"), Some("144"), no_randr, |_| {}).unwrap();
    assert_eq!(selected.factor, 2.0);
    assert_eq!(selected.source, "GDK_SCALE");
}

#[test]
fn fractional_xft_dpi_is_not_rounded() {
    for (dpi, factor) in [
        ("96", 1.0),
        ("120", 1.25),
        ("144", 1.5),
        ("168", 1.75),
        ("192", 2.0),
    ] {
        let selected = select_scale(None, None, Some(dpi), no_randr, |_| {}).unwrap();
        assert_eq!(selected.factor, factor);
        assert_eq!(selected.source, "Xft.dpi");
    }
}

#[test]
fn invalid_automatic_sources_warn_and_continue() {
    for invalid in ["", "garbage", "0", "-1", "NaN", "inf", "-inf", "1e-40"] {
        let mut warnings = Vec::new();
        let selected = select_scale(None, Some(invalid), Some("120"), no_randr, |warning| {
            warnings.push(warning)
        })
        .unwrap();
        assert_eq!(selected.factor, 1.25, "GDK_SCALE={invalid}");
        assert_eq!(selected.source, "Xft.dpi");
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("GDK_SCALE"));

        warnings.clear();
        let selected = select_scale(
            None,
            None,
            Some(invalid),
            || Some(1.5),
            |warning| warnings.push(warning),
        )
        .unwrap();
        assert_eq!(selected.factor, 1.5, "Xft.dpi={invalid}");
        assert_eq!(selected.source, "RandR");
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("Xft.dpi"));
    }
}

#[test]
fn invalid_explicit_override_remains_an_error() {
    for invalid in [
        "garbage", "0", "-1", "NaN", "inf", "-inf", "1e-40", " randr", "2 ",
    ] {
        let result = select_scale(Some(invalid), Some("2"), Some("144"), no_randr, |_| {});
        assert!(result.is_err(), "override={invalid}");
        assert!(result.unwrap_err().contains("GPUI_X11_SCALE_FACTOR"));
    }
}

#[test]
fn missing_desktop_sources_fall_back_to_randr_then_one() {
    let selected = select_scale(None, None, None, || Some(2.5), |_| {}).unwrap();
    assert_eq!(selected.factor, 2.5);
    assert_eq!(selected.source, "RandR");
    for randr in [
        None,
        Some(0.0),
        Some(-1.0),
        Some(f32::NAN),
        Some(f32::INFINITY),
        Some(f32::from_bits(1)),
    ] {
        let selected = select_scale(None, None, None, || randr, |_| {}).unwrap();
        assert_eq!(
            selected,
            ScaleSelection {
                factor: 1.0,
                source: "default"
            }
        );
    }
}
