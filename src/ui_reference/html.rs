use super::{
    contract::Snapshot,
    document::{Document, Kind, Node},
};
use std::{collections::BTreeMap, fmt::Write as _};

pub fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

// Only the font/theme shell is emitted without native measurements. There is
// deliberately no CSS component implementation to approximate native layout.
fn render(doc: &Document, s: &Snapshot) -> Result<String, String> {
    super::document::validate(doc, s)?;
    let mut fonts = String::new();
    for face in &s.font_faces {
        let data = &s.font_data[&face.path];
        let family = &face.family;
        let weight = face.weight;
        let style = &face.style;
        write!(fonts,"@font-face{{font-family:'{}';src:url(data:font/ttf;base64,{data});font-weight:{weight};font-style:{style};font-display:block;}}",escape(family)).unwrap();
    }
    Ok(format!("<!doctype html><html lang=\"en-US\"><meta charset=\"utf-8\"><title>Grove native component reference</title><style>{fonts}html,body{{margin:0;padding:0;width:{}px;height:{}px;overflow:hidden;font-family:'{}';font-size:{}px;background:{};}}svg{{display:block;}}svg text{{white-space:pre;}}</style><body><script></script></body></html>",doc.fixture.viewport.width,doc.fixture.viewport.height,s.ui_family,doc.fixture.rem,s.colors["background"]))
}

// Avoid Skia AA re-encoding an already identical opaque background. Native
// paint identity and geometry remain in the record and allocated SVG rectangle.
pub(super) fn elides_opaque_equal_background(p: &serde_json::Value) -> bool {
    let (Ok(fill), Ok(background), Ok(projected), Ok(projected_background)) = (
        serde_json::from_value::<[f32; 4]>(p["fill_rgba"].clone()),
        serde_json::from_value::<[f32; 4]>(p["background_rgba"].clone()),
        serde_json::from_value::<[u8; 3]>(p["html_fill"].clone()),
        serde_json::from_value::<[u8; 3]>(p["html_background"].clone()),
    ) else {
        return false;
    };
    p["kind"] == "rounded_rect"
        && background[3] == 1.
        && fill
            .iter()
            .chain(&background)
            .all(|v| v.is_finite() && (0. ..=1.).contains(v))
        && projected == projected_background
        && super::native::composite(fill, background) == projected
        && super::native::composite(background, background) == projected_background
}
fn box_node(id: &str, nodes: &BTreeMap<String, super::native::MeasuredNode>, svg: &mut String) {
    let n = &nodes[id];
    let b = &n.bounds;
    let elided = n.paint.as_ref().is_some_and(elides_opaque_equal_background);
    let marker = if elided {
        " data-paint-elision=\"opaque-equal-background\""
    } else {
        ""
    };
    let (fill, radius) = n
        .paint
        .as_ref()
        .filter(|p| p["kind"] == "rounded_rect" || p["kind"] == "surface")
        .map_or(("none".into(), 0.), |p| {
            (
                if elided {
                    "none".into()
                } else {
                    paint_color(&p["html_fill"])
                },
                p["radius"].as_f64().unwrap_or(0.),
            )
        });
    write!(svg,"<rect data-node=\"{}\" data-kind=\"{}\" x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" rx=\"{radius}\" fill=\"{fill}\"{marker}/>",escape(id),n.kind,b.x,b.y,b.width,b.height).unwrap();
    if n.kind == "icon" {
        let paint = n.paint.as_ref().unwrap();
        let source = paint["asset_svg"].as_str().unwrap();
        let color = paint_color(&paint["html_fill"]);
        svg.push_str(&source.replace("<svg ",&format!("<svg x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" style=\"color:{color}\" data-icon=\"{}\" ",b.x,b.y,b.width,b.height,escape(id))));
    }
    if let Some(t) = &n.text {
        let line = &t.lines[0];
        let x = t.glyphs[0].x;
        let color = paint_color(&n.paint.as_ref().unwrap()["html_fill"]);
        write!(svg,"<g data-text=\"{}\" data-layout-mode=\"native-measured\" aria-label=\"{}\" style=\"font-family:'{}';font-size:{}px;font-weight:{};font-style:{};line-height:{}px;fill:{color};\"><rect data-textbox x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"none\"/><rect data-linebox x=\"{x}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"none\"/>",escape(id),escape(&t.source_content),escape(&t.font_family),t.font_size,t.font_weight,t.font_style,t.line_height,t.bounds.x,t.bounds.y,t.bounds.width,t.bounds.height,line.y,line.width,line.height).unwrap();
        for glyph in &t.glyphs {
            write!(
                svg,
                "<text data-glyph x=\"{}\" y=\"{}\" xml:space=\"preserve\">{}</text>",
                glyph.x,
                t.baseline,
                escape(&glyph.text)
            )
            .unwrap();
        }
        svg.push_str("</g>");
    }
}
fn visit(n: &Node, nodes: &BTreeMap<String, super::native::MeasuredNode>, svg: &mut String) {
    box_node(&n.id, nodes, svg);
    if matches!(n.kind, Kind::CheckoutRow | Kind::DenseAction) {
        if n.icon.is_some() {
            box_node(&format!("{}-icon", n.id), nodes, svg);
        }
        box_node(&format!("{}-label", n.id), nodes, svg);
    }
    for child in &n.children {
        visit(child, nodes, svg);
    }
}

pub fn render_measured(
    doc: &Document,
    s: &Snapshot,
    record: &serde_json::Value,
) -> Result<String, String> {
    super::document::validate_measurement(doc, s, record)?;
    let nodes: BTreeMap<String, super::native::MeasuredNode> =
        serde_json::from_value(record["nodes"].clone()).map_err(|e| e.to_string())?;
    let mut svg=format!("<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{}\" height=\"{}\" viewBox=\"0 0 {} {}\" style=\"display:block;background:{}\">",doc.fixture.viewport.width,doc.fixture.viewport.height,doc.fixture.viewport.width,doc.fixture.viewport.height,s.colors["background"]);
    visit(&doc.root, &nodes, &mut svg);
    svg.push_str("</svg>");
    let metadata = serde_json::json!({"schema_version":1,"reference_hash":record["reference_hash"],"contract_hash":record["contract_hash"],"context":record["context"]});
    let metadata = serde_json::to_string(&metadata)
        .unwrap()
        .replace('<', "\\u003c");
    let base = render(doc, s)?;
    let start = base.find("<body>").unwrap() + 6;
    Ok(format!(
        "{}{svg}<script>window.__GROVE_REFERENCE__={metadata};</script></body></html>",
        &base[..start]
    ))
}
fn paint_color(v: &serde_json::Value) -> String {
    let c: [u8; 3] = serde_json::from_value(v.clone()).unwrap();
    format!("rgb({},{},{})", c[0], c[1], c[2])
}
