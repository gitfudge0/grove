use super::{
    contract::{hash, Snapshot},
    document::Document,
};
use crate::{
    theme as c,
    views::{components, rpx},
};
use gpui::{prelude::*, Bounds, Pixels, Window};
use gpui_component::button::Button;
use serde::{Deserialize, Serialize};
use std::{cell::RefCell, collections::BTreeMap, rc::Rc};

pub type Measurements = Rc<RefCell<BTreeMap<String, MeasuredNode>>>;
thread_local! {static OBSERVATION_ENABLED:std::cell::Cell<bool>=const {std::cell::Cell::new(true)};}
pub fn set_observation_enabled(enabled: bool) {
    OBSERVATION_ENABLED.set(enabled);
}
pub fn mutation() -> Option<String> {
    std::env::var("GROVE_REFERENCE_MUTATION").ok()
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}
impl From<Bounds<Pixels>> for Rect {
    fn from(b: Bounds<Pixels>) -> Self {
        Self {
            x: b.origin.x.into(),
            y: b.origin.y.into(),
            width: b.size.width.into(),
            height: b.size.height.into(),
        }
    }
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct MeasuredNode {
    pub kind: String,
    pub bounds: Rect,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<TextMetrics>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paint: Option<serde_json::Value>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TextLine {
    pub text: String,
    pub width: f32,
    pub y: f32,
    pub height: f32,
    pub baseline: f32,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Glyph {
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub width: f64,
    pub height: f32,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TextMetrics {
    pub source_content: String,
    pub color: String,
    pub content: String,
    pub font_family: String,
    pub font_size: f32,
    pub font_weight: u32,
    pub font_style: String,
    pub line_height: f32,
    pub bounds: Rect,
    pub baseline: f32,
    pub glyphs: Vec<Glyph>,
    pub lines: Vec<TextLine>,
}

fn build_hash() -> &'static str {
    static HASH: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    HASH.get_or_init(|| hash(fs_err::read(std::env::current_exe().unwrap()).unwrap()))
}
fn os_build() -> &'static str {
    static BUILD: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    BUILD.get_or_init(|| {
        let result = if cfg!(target_os = "macos") {
            std::process::Command::new("sw_vers")
                .args(["-buildVersion"])
                .output()
        } else if cfg!(target_os = "windows") {
            std::process::Command::new("cmd")
                .args(["/c", "ver"])
                .output()
        } else {
            std::process::Command::new("uname").args(["-srv"]).output()
        };
        let output = result.expect("observe native OS build");
        assert!(output.status.success());
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    })
}
pub fn context(doc: &Document, s: &Snapshot) -> serde_json::Value {
    serde_json::json!({"platform":std::env::consts::OS,"native_build_hash":build_hash(),"os_build":os_build(),"renderer":if cfg!(target_os="macos"){"metal-headless"}else{"unsupported-native-capture"},"native_engine":format!("gpui-lock-sha256:{}",s.owners["native-engine"]),"theme":doc.fixture.theme,"rem":doc.fixture.rem,"scale":doc.fixture.scale,"viewport":doc.fixture.viewport,"font_hashes":s.font_hashes,"motion":doc.fixture.motion,"locale":doc.fixture.locale})
}
pub fn composite(fg: [f32; 4], bg: [f32; 4]) -> [u8; 3] {
    std::array::from_fn(|i| {
        let blended = fg[i] * fg[3] + bg[i] * (1. - fg[3]);
        // Metal UNORM encoding rounds the scaled incoming float once. Scaling
        // again in f32 first introduces a second rounding near half-codepoints.
        // Preserve the native f32 blend; do not quantize its alpha.
        (f64::from(blended) * 255.)
            .round_ties_even()
            .clamp(0., 255.) as u8
    })
}
fn opaque(rgb: [u8; 3]) -> [f32; 4] {
    [
        f32::from(rgb[0]) / 255.,
        f32::from(rgb[1]) / 255.,
        f32::from(rgb[2]) / 255.,
        1.,
    ]
}
fn rgba(value: &serde_json::Value) -> [f32; 4] {
    serde_json::from_value(value.clone()).unwrap()
}
fn paint(
    n: &super::document::Node,
    bg: [f32; 4],
    nodes: &mut BTreeMap<String, MeasuredNode>,
    s: &Snapshot,
) {
    use super::document::Kind;
    let mut child_bg = bg;
    if n.kind == Kind::Surface {
        child_bg = opaque(composite(s.colors_rgba["rail"], bg));
    }
    if let Some(p) = nodes.get_mut(&n.id).and_then(|m| m.paint.as_mut()) {
        let fg = p
            .get("fill_rgba")
            .or_else(|| p.get("foreground_rgba"))
            .map(rgba)
            .unwrap();
        p["background_rgba"] = serde_json::json!(bg);
        p["html_background"] = serde_json::json!(composite(bg, bg));
        p["html_fill"] = serde_json::json!(composite(fg, bg));
        if n.kind == Kind::DenseAction {
            child_bg = opaque(composite(fg, bg));
        }
    }
    for id in [format!("{}-label", n.id), format!("{}-icon", n.id)] {
        let Some(m) = nodes.get_mut(&id) else {
            continue;
        };
        let Some(p) = m.paint.as_mut() else {
            continue;
        };
        let fg = rgba(&p["foreground_rgba"]);
        p["background_rgba"] = serde_json::json!(child_bg);
        p["html_background"] = serde_json::json!(composite(child_bg, child_bg));
        p["html_fill"] = serde_json::json!(composite(fg, child_bg));
        if let Some(text) = &m.text {
            let face = s
                .font_faces
                .iter()
                .find(|face| {
                    face.family == text.font_family
                        && u32::from(face.weight) == text.font_weight
                        && face.style == text.font_style
                })
                .expect("unsupported effective font face");
            let path = face.path.as_str();
            assert_eq!(
                text.font_family,
                crate::fonts::UI_FAMILY,
                "unsupported effective font family"
            );
            p["font_asset_sha256"] = serde_json::json!(s.font_hashes[path]);
        }
    }
    if let Some(m) = nodes.get_mut(&n.id) {
        if let (Some(text), Some(p)) = (&m.text, m.paint.as_mut()) {
            let face = s
                .font_faces
                .iter()
                .find(|face| {
                    face.family == text.font_family
                        && u32::from(face.weight) == text.font_weight
                        && face.style == text.font_style
                })
                .expect("unsupported effective font face");
            let path = face.path.as_str();
            p["font_asset_sha256"] = serde_json::json!(s.font_hashes[path]);
        }
    }
    for child in &n.children {
        paint(child, child_bg, nodes, s);
    }
}

pub fn record(doc: &Document, s: &Snapshot, measurements: &Measurements) -> serde_json::Value {
    let mut nodes = measurements.borrow().clone();
    paint(
        &doc.root,
        opaque(composite(
            s.colors_rgba["background"],
            s.colors_rgba["background"],
        )),
        &mut nodes,
        s,
    );
    serde_json::json!({"schema_version":1,"reference_hash":hash(serde_json::to_vec(doc).unwrap()),"contract_hash":s.contract_hash,"context":context(doc,s),"nodes":nodes,"interaction":{"status":"untested","cases":[]}})
}
thread_local! {static ACTIVE:RefCell<Option<Measurements>>=const {RefCell::new(None)};}
pub fn with_measurements<T>(measurements: &Measurements, render: impl FnOnce() -> T) -> T {
    struct Restore(Option<Measurements>);
    impl Drop for Restore {
        fn drop(&mut self) {
            ACTIVE.with(|active| *active.borrow_mut() = self.0.take());
        }
    }
    let previous = ACTIVE.with(|active| active.replace(Some(measurements.clone())));
    let _restore = Restore(previous);
    render()
}
fn active() -> Option<Measurements> {
    ACTIVE.with(|active| active.borrow().clone())
}
pub fn observe_icon(id: &str, name: &str, color: gpui::Hsla) {
    let Some(measurements) = active() else {
        return;
    };
    let source = crate::icons::svg_for_path(&format!("icons/{name}.svg")).expect("registered icon");
    let c: gpui::Rgba = color.into();
    measurements.borrow_mut().insert(id.into(),MeasuredNode{kind:"icon".into(),bounds:Rect{x:0.,y:0.,width:0.,height:0.},text:None,paint:Some(serde_json::json!({"kind":"svg","foreground_rgba":[c.r,c.g,c.b,c.a],"asset_svg":source,"asset_sha256":hash(source.as_bytes())}))});
}
pub fn observe_surface(id: &str, fill: gpui::Hsla) {
    let Some(measurements) = active() else {
        return;
    };
    let c: gpui::Rgba = fill.into();
    measurements.borrow_mut().insert(
        id.into(),
        MeasuredNode {
            kind: "surface".into(),
            bounds: Rect {
                x: 0.,
                y: 0.,
                width: 0.,
                height: 0.,
            },
            text: None,
            paint: Some(
                serde_json::json!({"kind":"surface","radius":0.,"fill_rgba":[c.r,c.g,c.b,c.a]}),
            ),
        },
    );
}
pub fn instrument_node(element: gpui::Div, id: &str, kind: &str) -> gpui::Div {
    let Some(measurements) = active() else {
        return element;
    };
    let name = id.to_owned();
    let kind = kind.to_owned();
    element.on_children_prepainted(move |bounds, _, _| {
        if let Some(bounds) = bounds.first() {
            let mut map = measurements.borrow_mut();
            let text = map.get(&name).and_then(|node| node.text.clone());
            let paint = map.get(&name).and_then(|node| node.paint.clone());
            map.insert(
                name.clone(),
                MeasuredNode {
                    kind: kind.clone(),
                    bounds: (*bounds).into(),
                    text,
                    paint,
                },
            );
        }
    })
}
fn record_text(
    id: &str,
    source: &str,
    layout: &gpui::TextLayout,
    measurements: &Measurements,
    window: &Window,
) {
    let content = layout.text();
    let lines = layout.line_layouts();
    assert_eq!(
        lines.len(),
        1,
        "only certified single-line text is supported"
    );
    let shaped = &lines[0];
    assert!(shaped.wrap_boundaries().is_empty());
    let bounds = layout.bounds();
    let height: f32 = layout.line_height().into();
    let baseline = f32::from(bounds.origin.y)
        + (height - f32::from(shaped.ascent()) - f32::from(shaped.descent())) / 2.
        + f32::from(shaped.ascent());
    let mut indices = std::collections::BTreeSet::new();
    for run in shaped.runs() {
        for glyph in &run.glyphs {
            indices.insert(glyph.index);
        }
    }
    let indices = indices.into_iter().collect::<Vec<_>>();
    let mut glyphs = Vec::new();
    for (i, index) in indices.iter().enumerate() {
        let end = indices.get(i + 1).copied().unwrap_or(content.len());
        let x: f32 = shaped.unwrapped_layout.x_for_index(*index).into();
        let right: f32 = shaped.unwrapped_layout.x_for_index(end).into();
        glyphs.push(Glyph {
            text: content[*index..end].to_owned(),
            x: f32::from(bounds.origin.x) + x,
            y: bounds.origin.y.into(),
            width: f64::from(right) - f64::from(x),
            height,
        });
    }
    // Logical widths use actual global Float32 endpoints, like the native
    // paint transform and observed browser SVG bounds; no local-width epsilon.
    let line_right = bounds.origin.x + shaped.width();
    let line_width = line_right - bounds.origin.x;
    for i in 0..glyphs.len() {
        let end = if let Some(next) = glyphs.get(i + 1) {
            f64::from(next.x)
        } else {
            f64::from(line_right)
        };
        glyphs[i].width = end - f64::from(glyphs[i].x);
    }
    let style = window.text_style();
    let expected_font = window.text_system().resolve_font(&style.font());
    assert!(
        shaped.runs().iter().all(|run| run.font_id == expected_font),
        "unrecorded fallback font is unsupported"
    );
    assert_eq!(
        style.font_features,
        gpui::FontFeatures::default(),
        "uncertified font features"
    );
    let rgba: gpui::Rgba = style.color.into();
    let color = format!(
        "rgba({:.6},{:.6},{:.6},{:.6})",
        rgba.r * 255.,
        rgba.g * 255.,
        rgba.b * 255.,
        rgba.a
    );
    let metrics = TextMetrics {
        source_content: source.into(),
        color,
        content: content.clone(),
        font_family: style.font_family.to_string(),
        font_size: shaped.font_size().into(),
        font_weight: style.font_weight.0 as u32,
        font_style: style.font_style.to_string().to_ascii_lowercase(),
        line_height: height,
        bounds: bounds.into(),
        baseline,
        glyphs,
        lines: vec![TextLine {
            text: content,
            width: line_width.into(),
            y: bounds.origin.y.into(),
            height,
            baseline,
        }],
    };
    measurements.borrow_mut().insert(
        id.to_owned(),
        MeasuredNode {
            kind: "text".into(),
            bounds: bounds.into(),
            text: Some(metrics),
            paint: Some(
                serde_json::json!({"kind":"text","foreground_rgba":[rgba.r,rgba.g,rgba.b,rgba.a]}),
            ),
        },
    );
}
pub fn instrument_text(
    element: gpui::Div,
    id: &str,
    value: &str,
    layout: gpui::TextLayout,
) -> gpui::Div {
    let Some(measurements) = active() else {
        return element;
    };
    let id = id.to_owned();
    let source = value.to_owned();
    element.on_children_prepainted(move |_, window, _| {
        record_text(&id, &source, &layout, &measurements, window);
    })
}
pub fn action_label(label: &str) -> String {
    if mutation().as_deref() == Some("missing-text") {
        format!("{label}!")
    } else {
        label.to_owned()
    }
}
pub fn instrument_action(
    button: Button,
    id: &str,
    label: &str,
    icon: Option<&str>,
    window: &Window,
) -> Button {
    let Some(measurements) = active() else {
        return button;
    };
    let name = id.to_owned();
    let observed_label = label.to_owned();
    let has_icon = icon.is_some();
    button.map(|button|match mutation().as_deref(){
            Some("font")=>button.font_weight(gpui::FontWeight::BOLD),
            Some("radius")=>button.rounded(gpui_component::button::ButtonRounded::Size(rpx(components::DENSE_ACTION_RECIPE.radius+1.).to_pixels(window.rem_size()))),
            Some("icon")=>button.icon(gpui_component::Icon::default().path("icons/check.svg")),
            Some("flat-fill")=>button.bg(c::RED()),
            _=>button,
        })
        .when(OBSERVATION_ENABLED.get(),|button|button.on_content_prepainted(move |metrics,window,_| {
            let bounds=metrics.bounds;let label_layout=metrics.label;
            if let Some(bg)=metrics.background {let rgba:gpui::Rgba=bg.into();measurements.borrow_mut().insert(name.clone(),MeasuredNode{kind:"dense_action".into(),bounds:Rect{x:0.,y:0.,width:0.,height:0.},text:None,paint:Some(serde_json::json!({"kind":"rounded_rect","radius":f32::from(metrics.radius),"fill_rgba":[rgba.r,rgba.g,rgba.b,rgba.a]}))});}
            if has_icon {if let Some(b)=bounds.first(){measurements.borrow_mut().insert(format!("{name}-icon"),MeasuredNode{kind:"icon".into(),bounds:(*b).into(),text:None,paint:metrics.icon_path.as_ref().map(|path|{let source=crate::icons::svg_for_path(path).expect("measured catalog icon");let c:gpui::Rgba=window.text_style().color.into();serde_json::json!({"kind":"svg","foreground_rgba":[c.r,c.g,c.b,c.a],"asset_svg":source,"asset_sha256":hash(source.as_bytes())})})});}}
            if let Some(layout)=label_layout{record_text(&format!("{name}-label"),&observed_label,&layout,&measurements,window);}
        }))
}
pub fn instrument_checkout(element: gpui::Div, label_id: &str) -> gpui::Div {
    let Some(measurements) = active() else {
        return element;
    };
    let label_id = label_id.to_owned();
    element.on_children_prepainted(move |bounds, _, _| {
        if let Some(bounds) = bounds.get(1) {
            if let Some(node) = measurements.borrow_mut().get_mut(&label_id) {
                node.bounds = (*bounds).into();
            }
        }
    })
}
