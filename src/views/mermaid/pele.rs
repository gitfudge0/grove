//! Bounded, offline Pele execution and SVG materialization for resvg.
use resvg::usvg::fontdb;
use rquickjs::{Context, Function, Runtime};
use std::{
    collections::HashMap,
    sync::{Arc, OnceLock},
    time::{Duration, Instant},
};

const BUNDLE: &str = include_str!("../../../vendor/pele/pele.global.js");
const JS_MEMORY_BYTES: usize = 64 * 1024 * 1024;
const MAX_SVG_BYTES: usize = 512 * 1024;
const RENDER_TIMEOUT: Duration = Duration::from_secs(1);

pub(super) fn fonts() -> Arc<fontdb::Database> {
    static FONTS: OnceLock<Arc<fontdb::Database>> = OnceLock::new();
    FONTS
        .get_or_init(|| {
            let mut db = fontdb::Database::new();
            // Match the rasterizer and retain platform fallback for unsupported glyphs.
            db.load_system_fonts();
            for name in [
                "DMSans-Regular.ttf",
                "DMSans-Bold.ttf",
                "JetBrainsMono-Regular.ttf",
                "JetBrainsMono-Bold.ttf",
                "JetBrainsMono-Italic.ttf",
                "JetBrainsMono-BoldItalic.ttf",
            ] {
                if let Some(font) = crate::assets::Assets::get(&format!("fonts/{name}")) {
                    db.load_font_data(font.data.into_owned());
                }
            }
            Arc::new(db)
        })
        .clone()
}

fn measure(
    db: &Arc<fontdb::Database>,
    text: &str,
    size: f32,
    style: u32,
    deadline: Instant,
) -> Option<f32> {
    let size = if style & 0x18 != 0 { size / 1.2 } else { size };
    let family = if style & 4 != 0 {
        crate::fonts::MONO_FAMILY
    } else {
        crate::fonts::UI_FAMILY
    };
    let base = db.query(&fontdb::Query {
        families: &[fontdb::Family::Name(family)],
        weight: fontdb::Weight(if style & 1 != 0 { 700 } else { 400 }),
        style: if style & 2 != 0 {
            fontdb::Style::Italic
        } else {
            fontdb::Style::Normal
        },
        ..fontdb::Query::default()
    })?;
    let supported = db.with_face_data(base, |data, index| {
        let face = rustybuzz::Face::from_slice(data, index)?;
        Some(
            text.chars()
                .map(|ch| (ch, face.glyph_index(ch).is_some()))
                .collect::<Vec<_>>(),
        )
    })??;
    let mut fallback_ids = HashMap::new();
    let mut runs: Vec<(fontdb::ID, String)> = Vec::new();
    for (ch, present) in supported {
        if Instant::now() >= deadline {
            return None;
        }
        let id = if present {
            base
        } else if let Some(id) = fallback_ids.get(&ch) {
            *id
        } else {
            let mut selected = base;
            let base_face = db.face(base)?;
            // Use resvg's fallback selection rule, with deadline checks while
            // scanning platform fonts instead of an uninterruptible callback.
            for face in db.faces() {
                if Instant::now() >= deadline {
                    return None;
                }
                if face.id == base
                    || (face.style != base_face.style
                        && face.weight != base_face.weight
                        && face.stretch != base_face.stretch)
                {
                    continue;
                }
                if db
                    .with_face_data(face.id, |data, index| {
                        rustybuzz::Face::from_slice(data, index)
                            .is_some_and(|f| f.glyph_index(ch).is_some())
                    })
                    .unwrap_or(false)
                {
                    selected = face.id;
                    break;
                }
            }
            fallback_ids.insert(ch, selected);
            selected
        };
        if let Some((_, run)) = runs.last_mut().filter(|(last, _)| *last == id) {
            run.push(ch);
        } else {
            runs.push((id, ch.to_string()));
        }
    }
    let mut width = 0.0;
    for (id, run) in runs {
        if Instant::now() >= deadline {
            return None;
        }
        width += db.with_face_data(id, |data, index| {
            let face = rustybuzz::Face::from_slice(data, index)?;
            let mut buffer = rustybuzz::UnicodeBuffer::new();
            buffer.push_str(&run);
            buffer.guess_segment_properties();
            let shaped = rustybuzz::shape(&face, &[], buffer);
            let advance: i32 = shaped.glyph_positions().iter().map(|p| p.x_advance).sum();
            Some(advance as f32 * size / face.units_per_em() as f32)
        })??;
    }
    Some(width)
}

pub(super) fn render(
    source: &str,
    font_size: f32,
    palette: &HashMap<String, String>,
    now: u64,
) -> Option<String> {
    let db = fonts();
    let runtime = Runtime::new().ok()?;
    runtime.set_memory_limit(JS_MEMORY_BYTES);
    runtime.set_max_stack_size(1024 * 1024);
    let deadline = Instant::now() + RENDER_TIMEOUT;
    runtime.set_interrupt_handler(Some(Box::new(move || Instant::now() >= deadline)));
    let context = Context::full(&runtime).ok()?;
    let svg = context.with(|ctx| {
        ctx.eval::<(), _>(BUNDLE).ok()?;
        let width = Function::new(ctx.clone(), move |text: String, size: f32, style: u32| {
            if Instant::now() >= deadline || text.len() > super::MAX_SOURCE_BYTES {
                return f32::NAN;
            }
            measure(&db, &text, size, style, deadline).unwrap_or(f32::NAN)
        })
        .ok()?;
        ctx.globals().set("groveMeasure", width).ok()?;
        // Arguments cross the JS boundary as data, never executable source.
        ctx.eval::<(), _>(
            r"globalThis.groveRender=(source,size,now)=>pele.render(source,{
            fontFamily:'DM Sans',fontFamilyMono:'JetBrains Mono',fontSize:size,
            measurer:{width:groveMeasure},autoDirection:false,responsive:false,
            limit:8192,outputLimit:524288,maxEdges:100,links:false,images:false,
            linkSchemes:[],imageSchemes:[],now
        }).svg",
        )
        .ok()?;
        let render: Function = ctx.globals().get("groveRender").ok()?;
        render.call::<_, String>((source, font_size, now)).ok()
    })?;
    if svg.len() > MAX_SVG_BYTES {
        return None;
    }
    materialize(&svg, palette, font_size)
}

// Pele emits CSS variables in attributes and inline style. Resolve only XML
// attribute values: diagram labels containing literal `var(...)` remain data.
fn resolve(value: &str, vars: &HashMap<String, String>) -> Option<String> {
    let mut value = value.to_owned();
    for _ in 0..32 {
        let Some(start) = value.rfind("var(") else {
            return mix(&value);
        };
        let end = start + value[start..].find(')')?;
        let (name, fallback) = value[start + 4..end]
            .split_once(',')
            .unwrap_or((&value[start + 4..end], ""));
        let replacement = vars
            .get(name.trim())
            .map_or(fallback.trim(), String::as_str)
            .to_owned();
        if replacement.is_empty() {
            return None;
        }
        value.replace_range(start..=end, &replacement);
    }
    None
}

fn mix(value: &str) -> Option<String> {
    let Some(args) = value
        .strip_prefix("color-mix(in srgb,")
        .and_then(|s| s.strip_suffix(')'))
    else {
        return Some(value.to_owned());
    };
    let (first, second) = args.split_once(',')?;
    let (first, percent) = first.trim().rsplit_once(' ')?;
    let ratio = percent.strip_suffix('%')?.parse::<f32>().ok()? / 100.;
    let a = first.parse::<svgtypes::Color>().ok()?;
    let b = second.trim().parse::<svgtypes::Color>().ok()?;
    let channel = |a: u8, b: u8| (f32::from(a) * ratio + f32::from(b) * (1. - ratio)).round() as u8;
    Some(format!(
        "#{:02x}{:02x}{:02x}",
        channel(a.red, b.red),
        channel(a.green, b.green),
        channel(a.blue, b.blue)
    ))
}

fn materialize(svg: &str, palette: &HashMap<String, String>, font_size: f32) -> Option<String> {
    let doc = roxmltree::Document::parse(svg).ok()?;
    if doc.descendants().count() > 8192 {
        return None;
    }
    // Pretty printing inserts significant whitespace into mixed SVG text:
    // xml:space=preserve then changes text-anchor and displaces styled labels.
    let mut writer = xmlwriter::XmlWriter::new(xmlwriter::Options {
        indent: xmlwriter::Indent::None,
        ..xmlwriter::Options::default()
    });
    write_node(doc.root_element(), &mut writer, palette, font_size, 0)?;
    Some(writer.end_document())
}

fn write_node(
    node: roxmltree::Node<'_, '_>,
    writer: &mut xmlwriter::XmlWriter,
    inherited: &HashMap<String, String>,
    font_size: f32,
    depth: usize,
) -> Option<()> {
    if depth > 64 {
        return None;
    }
    if node.is_text() {
        writer.write_text(node.text()?);
        return Some(());
    }
    if !node.is_element() {
        return Some(());
    }
    // No external or executable content is part of the raster contract.
    if matches!(
        node.tag_name().name(),
        "script" | "foreignObject" | "image" | "a" | "style"
    ) {
        return None;
    }
    let mut vars = inherited.clone();
    let style = node.attribute("style").unwrap_or("");
    for (key, value) in style.split(';').filter_map(|s| s.split_once(':')) {
        if key.starts_with("--") {
            vars.insert(key.to_owned(), value.to_owned());
        }
    }
    writer.start_element(node.tag_name().name());
    if depth == 0 {
        writer.write_attribute("xmlns", "http://www.w3.org/2000/svg");
        writer.write_attribute("font-size", &font_size);
    }
    for attr in node.attributes() {
        if attr.name().starts_with("on") || attr.name() == "href" {
            return None;
        }
        if attr.name() == "style" {
            let mut resolved = String::new();
            for (key, value) in style.split(';').filter_map(|s| s.split_once(':')) {
                if key.starts_with("--") || key == "font" {
                    continue;
                }
                let value = resolve(value, &vars)?;
                resolved.push_str(key);
                resolved.push(':');
                resolved.push_str(&if key == "font-family" {
                    host_family(&value).to_owned()
                } else {
                    value
                });
                resolved.push(';');
            }
            writer.write_attribute("style", &resolved);
        } else {
            let value = resolve(attr.value(), &vars)?;
            let name = if attr.namespace() == Some("http://www.w3.org/XML/1998/namespace") {
                format!("xml:{}", attr.name())
            } else {
                attr.name().to_owned()
            };
            writer.write_attribute(
                &name,
                &if attr.name() == "font-family" {
                    host_family(&value).to_owned()
                } else {
                    value
                },
            );
        }
    }
    for child in node.children() {
        write_node(child, writer, &vars, font_size, depth + 1)?;
    }
    writer.end_element();
    Some(())
}

fn host_family(value: &str) -> &'static str {
    if value.contains("JetBrains Mono") || value == "monospace" {
        crate::fonts::MONO_FAMILY
    } else {
        crate::fonts::UI_FAMILY
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn css_materialization_preserves_text_and_resolves_nested_values() {
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="50" style="--_fg:var(--pele-fg,#fff);color:var(--_fg)"><text xml:space="preserve" fill="var(--_fg)">var(--keep,label)</text></svg>"#;
        let palette = HashMap::from([("--pele-fg".to_owned(), "#123456".to_owned())]);
        let result = materialize(svg, &palette, 13.).unwrap();
        assert!(result.contains("fill=\"#123456\""));
        assert!(result.contains("var(--keep,label)"));
        assert!(result.contains("xml:space=\"preserve\""));
        assert_eq!(
            mix("color-mix(in srgb,#ffffff 50%,#000000)").unwrap(),
            "#808080"
        );
        assert!(resolve("var(--unknown)", &palette).is_none());
    }
    #[test]
    fn materialization_does_not_inject_whitespace_into_styled_labels() {
        let source = r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="50"><text xml:space="preserve" text-anchor="middle"><tspan font-family="var(--_fm,JetBrains Mono)">source</tspan></text></svg>"#;
        let svg = materialize(source, &HashMap::new(), 13.).unwrap();
        let doc = roxmltree::Document::parse(&svg).unwrap();
        let text = doc
            .descendants()
            .find(|node| node.has_tag_name("text"))
            .unwrap();
        let content = text
            .descendants()
            .filter(roxmltree::Node::is_text)
            .filter_map(|node| node.text())
            .collect::<String>();
        assert_eq!(content, "source");
    }
    #[test]
    fn engine_interrupts_and_has_no_host_io() {
        let runtime = Runtime::new().unwrap();
        runtime.set_memory_limit(JS_MEMORY_BYTES);
        let deadline = Instant::now() + Duration::from_millis(20);
        runtime.set_interrupt_handler(Some(Box::new(move || Instant::now() >= deadline)));
        let context = Context::full(&runtime).unwrap();
        context.with(|ctx| {
            assert!(ctx.eval::<bool, _>("typeof fetch==='undefined' && typeof document==='undefined' && typeof require==='undefined'").unwrap());
            assert!(ctx.eval::<(), _>("while(true){}").is_err());
        });
        assert!(Instant::now() < deadline + Duration::from_secs(1));
    }
    #[test]
    fn bundled_metrics_follow_bold_mono_and_subscript_styles() {
        let db = fonts();
        let deadline = Instant::now() + Duration::from_secs(2);
        let width = |style| measure(&db, "Hello WWW", 16., style, deadline).unwrap();
        assert!((width(8) - width(0) / 1.2).abs() < 0.001);
        assert!((width(16) - width(0) / 1.2).abs() < 0.001);
        assert_ne!(width(0), width(1));
        assert_ne!(width(0), width(4));
        assert!(measure(
            &db,
            "Hello",
            16.,
            0,
            Instant::now().checked_sub(Duration::from_secs(1)).unwrap()
        )
        .is_none());
    }
    #[test]
    fn actual_bundle_disables_links() {
        let source = "flowchart LR\nA[Hello]-->B[World]\nclick A href \"https://example.com\"";
        let svg = render(source, 13., &HashMap::new(), 0).unwrap();
        assert!(!svg.contains("href="));
        assert!(svg.contains("DM Sans"));
    }
    #[test]
    fn actual_bundle_disables_images_and_ignores_source_font_config() {
        let source = r#"flowchart LR
A@{ img: "https://example.com/image.png", label: "Image" }"#;
        let svg = render(source, 13., &HashMap::new(), 0).unwrap();
        assert!(!svg.contains("<image"));
        assert!(!svg.contains("https://example.com"));
        let source = "---\nconfig:\n  fontFamily: serif\n  themeVariables:\n    fontFamily: serif\n---\nflowchart LR\nA[Hello]-->B[World]";
        let svg = render(source, 13., &HashMap::new(), 0).unwrap();
        assert!(svg.contains("font-family:DM Sans"));
        assert!(!svg.contains("serif"));
    }
}
