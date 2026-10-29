use super::{
    contract,
    document::{self, Document},
    generate, html,
};
use std::io::Write as _;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

#[cfg(grove_reference_capture)]
use super::native;
#[cfg(grove_reference_capture)]
use gpui::{prelude::FluentBuilder, InteractiveElement, Styled};
#[cfg(grove_reference_capture)]
use std::{cell::RefCell, rc::Rc};

#[cfg(grove_reference_capture)]
mod generated {
    include!(env!("GROVE_REFERENCE_GENERATED"));
}

// This include is intentionally present in ordinary cfg(test), without the
// capture cfg. It compiles exactly the full generated integration API.
mod ordinary_generated {
    include!("generated_test_composition.rs");
}
const _: fn(&ordinary_generated::Bindings, &mut gpui::Window, &mut gpui::App) -> gpui::AnyElement =
    ordinary_generated::render;

#[test]
fn generated_composition_compiles_with_ordinary_test_bindings() {
    let snapshot = contract::export("grove-dark").unwrap();
    let doc = document::pilot(&snapshot);
    let current = generate::generate(&doc, &snapshot).unwrap();
    // Provenance comments change with native owner bytes. The composition body
    // must remain exact; source/contract fingerprint tests cover provenance.
    assert_eq!(
        current.lines().skip(3).collect::<Vec<_>>(),
        include_str!("generated_test_composition.rs")
            .lines()
            .skip(3)
            .collect::<Vec<_>>()
    );
    assert_eq!(ordinary_generated::LAYOUT_HASH, generate::layout_hash(&doc));
    let bindings = ordinary_generated::Bindings::fixture(std::rc::Rc::new(|_, _, _| {}));
    assert_eq!(bindings.run_label.as_ref(), "Run");
}

fn output() -> PathBuf {
    PathBuf::from(std::env::var("GROVE_REFERENCE_OUTPUT").expect("GROVE_REFERENCE_OUTPUT"))
}
fn write(path: impl AsRef<Path>, content: impl AsRef<[u8]>) {
    fs_err::write(path, content).unwrap();
}
fn read_document() -> Document {
    serde_json::from_slice(
        &fs_err::read(std::env::var("GROVE_REFERENCE_DOCUMENT").expect("GROVE_REFERENCE_DOCUMENT"))
            .unwrap(),
    )
    .unwrap()
}
fn expected_artifacts(
    doc: &Document,
    s: &contract::Snapshot,
    measured: Option<&serde_json::Value>,
) -> BTreeMap<String, Vec<u8>> {
    let mut result: BTreeMap<String, Vec<u8>> = BTreeMap::from([
        (
            "document.json".into(),
            serde_json::to_vec_pretty(doc).unwrap(),
        ),
        (
            "snapshot.json".into(),
            serde_json::to_vec_pretty(s).unwrap(),
        ),
        (
            "generated.rs".into(),
            generate::generate(doc, s).unwrap().into_bytes(),
        ),
    ]);
    if let Some(record) = measured {
        result.insert(
            "measured-layout.json".into(),
            serde_json::to_vec_pretty(record).unwrap(),
        );
        result.insert(
            "reference.html".into(),
            html::render_measured(doc, s, record).unwrap().into_bytes(),
        );
    }
    let hashes: BTreeMap<_, _> = result
        .iter()
        .map(|(name, bytes)| (name.clone(), contract::hash(bytes)))
        .collect();
    let manifest = serde_json::json!({"schema_version":1,"render_mode":if measured.is_some(){"native-measured-svg"}else{"bootstrap"},"generator_version":contract::GENERATOR_VERSION,"reference_hash":contract::hash(serde_json::to_vec(doc).unwrap()),"layout_hash":generate::layout_hash(doc),"contract_hash":s.contract_hash,"node_to_native_selector_map":generate::selector_map(doc),"outputs":hashes});
    result.insert(
        "manifest.json".into(),
        serde_json::to_vec_pretty(&manifest).unwrap(),
    );
    result
}
fn check_artifacts(doc: &Document, s: &contract::Snapshot, out: &Path) -> Result<(), String> {
    let measured = fs_err::read(out.join("measured-layout.json"))
        .ok()
        .map(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes))
        .transpose()
        .map_err(|e| e.to_string())?;
    for (name, bytes) in expected_artifacts(doc, s, measured.as_ref()) {
        let existing = fs_err::read(out.join(&name)).map_err(|e| e.to_string())?;
        if existing != bytes {
            return Err(format!("stale or edited generated artifact: {name}"));
        }
    }
    Ok(())
}
fn artifacts(doc: &Document, out: &Path) -> contract::Snapshot {
    assert!(
        !out.join("reference-lock.json").exists(),
        "immutable reference already frozen; choose a new revision directory"
    );
    let s = contract::export(&doc.fixture.theme).unwrap();
    document::validate(doc, &s).unwrap();
    fs_err::create_dir_all(out).unwrap();
    for (name, bytes) in expected_artifacts(doc, &s, None) {
        write(out.join(name), bytes);
    }
    s
}
fn freeze(doc: &Document, s: &contract::Snapshot, record: &serde_json::Value, out: &Path) {
    assert!(
        !out.join("reference-lock.json").exists(),
        "cannot overwrite frozen reference"
    );
    let mut hashes = BTreeMap::new();
    for (name, bytes) in expected_artifacts(doc, s, Some(record)) {
        hashes.insert(name.clone(), contract::hash(&bytes));
        write(out.join(name), bytes);
    }
    let lock = serde_json::json!({"schema_version":1,"render_mode":"native-measured-svg","reference_hash":record["reference_hash"],"contract_hash":s.contract_hash,"context":record["context"],"artifacts":hashes});

    let mut f = fs_err::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(out.join("reference-lock.json"))
        .unwrap();
    f.write_all(&serde_json::to_vec_pretty(&lock).unwrap())
        .unwrap();
}
fn locked_snapshot(
    doc: &Document,
    out: &Path,
) -> Result<(contract::Snapshot, serde_json::Value), String> {
    let lock: serde_json::Value = serde_json::from_slice(
        &fs_err::read(out.join("reference-lock.json")).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    if lock["schema_version"] != 1
        || lock["render_mode"] != "native-measured-svg"
        || lock["reference_hash"] != contract::hash(serde_json::to_vec(doc).unwrap())
    {
        return Err("invalid frozen reference identity".into());
    }
    for (name, expected) in lock["artifacts"]
        .as_object()
        .ok_or("missing locked artifacts")?
    {
        if ![
            "document.json",
            "snapshot.json",
            "generated.rs",
            "manifest.json",
            "reference.html",
            "measured-layout.json",
        ]
        .contains(&name.as_str())
        {
            return Err("invalid reference artifact path".into());
        }
        let bytes = fs_err::read(out.join(name)).map_err(|e| e.to_string())?;
        if expected.as_str() != Some(contract::hash(bytes).as_str()) {
            return Err(format!("frozen reference artifact changed: {name}"));
        }
    }
    if lock["artifacts"].as_object().unwrap().len() != 6 {
        return Err("incomplete frozen reference".into());
    }
    let s: contract::Snapshot = serde_json::from_slice(
        &fs_err::read(out.join("snapshot.json")).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let fresh = contract::export(&doc.fixture.theme)?;
    document::validate(doc, &fresh)?;
    if s.contract_hash != fresh.contract_hash || contract::fingerprint(&s) != fresh.contract_hash {
        return Err("stale frozen design system; author a new reference revision".into());
    }
    check_artifacts(doc, &s, out)?;
    Ok((s, lock))
}

// JSON encoders may spell an exact context value 16 or 16.0. Compare
// numeric values exactly; do not apply geometry tolerances or normalize inputs.
fn same_context(a: &serde_json::Value, b: &serde_json::Value) -> bool {
    use serde_json::Value;
    match (a, b) {
        (Value::Number(a), Value::Number(b)) => a.as_f64() == b.as_f64(),
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| same_context(a, b))
        }
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len()
                && a.iter()
                    .all(|(key, a)| b.get(key).is_some_and(|b| same_context(a, b)))
        }
        _ => a == b,
    }
}
#[test]
fn context_representation_preserves_exact_environment_values() {
    let a: serde_json::Value =
        serde_json::from_str(r#"{"rem":16.0,"viewport":{"width":420.0},"theme":"grove-dark"}"#)
            .unwrap();
    let b = serde_json::json!({"rem":16,"viewport":{"width":420},"theme":"grove-dark"});
    assert!(same_context(&a, &b));
    let mut changed = b.clone();
    changed["rem"] = serde_json::json!(16.000001);
    assert!(!same_context(&a, &changed));
    changed = b.clone();
    changed["theme"] = serde_json::json!("grove-light");
    assert!(!same_context(&a, &changed));
}
fn check_certificate(doc: &Document, out: &Path) -> Result<(), String> {
    let (snapshot, lock) = locked_snapshot(doc, out)?;
    let read = |name: &str| -> Result<Vec<u8>, String> {
        fs_err::read(out.join(name)).map_err(|e| e.to_string())
    };
    let certificate: serde_json::Value =
        serde_json::from_slice(&read("certificate.json")?).map_err(|e| e.to_string())?;
    let native_bytes = read("native.json")?;
    let record: serde_json::Value =
        serde_json::from_slice(&native_bytes).map_err(|e| e.to_string())?;
    if certificate["schema_version"] != 1 || certificate["status"] != "certified" {
        return Err(
            "reference is not certified; all paired and native interaction gates must pass".into(),
        );
    }
    for gate in [
        "source_validation",
        "font_bindings",
        "paint",
        "geometry",
        "raster",
        "interaction",
        "verifier",
    ] {
        if certificate[gate]["status"] != "passed" {
            return Err(format!("uncertified gate: {gate}"));
        }
    }
    if certificate["reference_hash"] != lock["reference_hash"]
        || certificate["contract_hash"] != snapshot.contract_hash
        || !same_context(&certificate["reference_context"], &lock["context"])
        || !same_context(&certificate["context"], &record["context"])
        || !same_context(&record["reference_context"], &lock["context"])
        || certificate["native_hash"] != contract::hash(&native_bytes)
        || certificate["html_hash"] != contract::hash(read("reference.html")?)
    {
        return Err("certificate reference/candidate identity mismatch".into());
    }
    if certificate["verifier"]["version"] != 1
        || certificate["verifier"]["sha256"]
            != contract::hash(include_bytes!("../../scripts/verify-ui-reference.mjs"))
    {
        return Err("verifier changed; rerun paired verification".into());
    }
    let hashes = certificate["artifacts"]
        .as_object()
        .ok_or("certificate has no evidence hashes")?;
    for name in [
        "native.json",
        "native.png",
        "reference.html",
        "measured-layout.json",
        "document.json",
        "snapshot.json",
        "generated.rs",
        "manifest.json",
        "reference-lock.json",
        "browser.json",
        "browser.png",
        "geometry-diff.json",
        "raster-diff.json",
        "diff.png",
        "overlay.png",
    ] {
        if !hashes.contains_key(name) {
            return Err(format!("missing evidence: {name}"));
        }
    }
    for (name, expected) in hashes {
        if name.is_empty()
            || name.contains(['/', '\\'])
            || name == "."
            || name == ".."
            || expected.as_str() != Some(contract::hash(read(name)?).as_str())
        {
            return Err(format!("edited or unsafe certificate artifact: {name}"));
        }
    }
    if record["context"] != super::native::context(doc, &snapshot) {
        return Err("native build or environment changed; capture and verify again".into());
    }
    document::validate_measurement(doc, &snapshot, &record)?;
    Ok(())
}

#[test]
#[ignore = "explicit reference authoring tool; never run as part of ordinary tests"]
fn tool() {
    let command = std::env::var("GROVE_REFERENCE_COMMAND").unwrap();
    let out = output();
    if command != "check" && command != "validate" {
        fs_err::create_dir_all(&out).unwrap();
    }
    if command == "export" || command == "init" {
        let theme = std::env::var("GROVE_REFERENCE_THEME").unwrap_or("grove-dark".into());
        let s = contract::export(&theme).unwrap();
        write(
            out.join("snapshot.json"),
            serde_json::to_vec_pretty(&s).unwrap(),
        );
        if command == "init" {
            let d = document::pilot(&s);
            write(
                out.join("document.json"),
                serde_json::to_vec_pretty(&d).unwrap(),
            );
            artifacts(&d, &out);
        }
    } else {
        let doc = read_document();
        if command == "ready" {
            check_certificate(&doc, &out).expect("implementation-ready gate failed");
        } else if command == "check" {
            let _ = locked_snapshot(&doc, &out).unwrap();
        } else if command == "validate" {
            let s = contract::export(&doc.fixture.theme).unwrap();
            document::validate(&doc, &s).unwrap();
        } else if command == "render" {
            let _=locked_snapshot(&doc,&out).expect("render requires a frozen native-measured reference; run author in a new revision directory");
        } else {
            artifacts(&doc, &out);
        }
    }
    println!("GROVE_REFERENCE: {command}: {}", out.display());
}

#[test]
fn contract_is_reproducible_and_mutations_invalidate() {
    let s = contract::export("grove-dark").unwrap();
    let again = contract::export("grove-dark").unwrap();
    assert_eq!(
        serde_json::to_vec(&s).unwrap(),
        serde_json::to_vec(&again).unwrap()
    );
    let d = document::pilot(&s);
    document::validate(&d, &s).unwrap();
    let loading = s.dense_action["loading_icon"].as_str().unwrap();
    assert_eq!(
        loading,
        crate::views::components::DENSE_ACTION_RECIPE.loading_icon
    );
    assert!(
        s.icons[loading].contains("<path"),
        "shared loading icon must be a real catalog glyph"
    );
    for field in ["tokens", "colors", "font_hashes", "dense_action", "owners"] {
        let mut changed = s.clone();
        match field {
            "tokens" => {
                *changed.tokens.get_mut("space_lg").unwrap() += 1.;
            }
            "colors" => {
                changed
                    .colors
                    .insert("foreground".into(), "rgba(0,0,0,1)".into());
            }
            "font_hashes" => {
                changed
                    .font_hashes
                    .insert("fonts/DMSans-Regular.ttf".into(), "changed".into());
            }
            "owners" => {
                changed
                    .owners
                    .insert("component-recipe".into(), "changed".into());
            }
            _ => {
                changed.dense_action["padding_x"] = serde_json::json!(99);
            }
        }
        changed.contract_hash = contract::fingerprint(&changed);
        assert_ne!(changed.contract_hash, s.contract_hash, "mutation {field}");
        let owner = match field {
            "tokens" => "resolved/src/views/tokens.rs",
            "colors" => "resolved/src/theme.rs",
            "font_hashes" => "resolved/src/fonts.rs",
            "dense_action" => "resolved/src/views/components.rs",
            _ => "component-recipe",
        };
        let diagnostic = document::validate(&d, &changed).unwrap_err();
        assert!(
            diagnostic.contains(owner),
            "mutation {field} must identify its exact native owner: {diagnostic}"
        );
        // Injection exercises captured native inputs, not physical source writes.
        let restored = contract::export("grove-dark").unwrap();
        assert_eq!(
            serde_json::to_vec(&restored).unwrap(),
            serde_json::to_vec(&s).unwrap()
        );
    }
    let mut advanced_head = s.clone();
    advanced_head.source_revision = "new-commit-with-identical-native-owners".into();
    assert_eq!(contract::fingerprint(&advanced_head), s.contract_hash);
    document::validate(&d, &advanced_head).unwrap();
    assert_eq!(contract::fingerprint(&s), s.contract_hash);
}
#[test]
fn document_rejects_unknown_properties_and_invalid_translation() {
    let s = contract::export("grove-dark").unwrap();
    let d = document::pilot(&s);
    for (property, value) in [("kind", "unknown_recipe"), ("state", "unknown_variant")] {
        let mut invalid = serde_json::to_value(&d).unwrap();
        invalid["root"][property] = serde_json::json!(value);
        assert!(
            serde_json::from_value::<Document>(invalid).is_err(),
            "unknown component/state must fail schema"
        );
    }
    let mut raw = serde_json::to_value(&d).unwrap();
    raw["root"]["css"] = serde_json::json!("padding:13px");
    assert!(serde_json::from_value::<Document>(raw).is_err());
    for mutation in 0..16 {
        let mut bad = d.clone();
        match mutation {
            0 => bad.root.children[0].id = bad.root.id.clone(),
            1 => bad.root.layout.gap = "13px".into(),
            2 => bad.root.children[0].children[0].action = Some("missing".into()),
            3 => bad.actions[0] = "type".into(),
            4 => bad.actions[0] = "not-rust".into(),
            5 => bad.root.layout.text_policy = document::TextPolicy::Wrap,
            6 => bad.root.children[0].id = "action-rest-label".into(),
            7 => bad.root.children[0].icon = Some("not-an-icon".into()),
            8 => bad.root.children[0].children[0].state = document::State::Hover,
            9 => bad.root.children[0].children[0].state = document::State::Focus,
            10 => {
                bad.values.insert("unused".into(), "unused".into());
            }
            11 => {
                bad.root.children[0].text = Some("unsupported 🦄".into());
            }
            12 => {
                bad.root.text = Some("ignored".into());
                bad.root.text_binding = Some("ignored_label".into());
                bad.values.insert("ignored_label".into(), "ignored".into());
            }
            13 => {
                bad.root
                    .children
                    .retain(|row| row.children[0].state != document::State::Loading);
            }
            14 => {
                bad.root.children[2].children[0].icon = None;
            }
            _ => {
                bad.root.children[0].children[0].text_binding = Some("missing_value".into());
            }
        }
        assert!(document::validate(&bad, &s).is_err(), "mutation {mutation}");
    }
    assert_eq!(
        generate::generate_dense_action().unwrap(),
        include_str!("generated_dense_action.rs"),
        "generated production recipe edited; regenerate it"
    );
    let map = generate::selector_map(&d);
    assert_eq!(map["pilot"]["owner_selector"], "ui-reference-pilot");
    assert_eq!(
        map["action-rest-label"]["owner_selector"],
        "ui-reference-action-rest"
    );
    assert_eq!(
        map["action-rest-label"]["measurement"],
        "vendor-label-text-layout"
    );
    assert_eq!(
        map.len(),
        19,
        "all authored and reserved pilot identities must map to native owners/slots"
    );
    let generated = generate::generate(&d, &s).unwrap();
    assert!(generated.contains("native::action"));
    assert!(generated.contains("bindings.run.clone()"));
    assert_eq!(generated, generate::generate(&d, &s).unwrap());
    let mut hostile = d.clone();
    hostile.root.children[0].text = Some("</script><img onerror='bad'>".into());
    let escaped = html::escape(hostile.root.children[0].text.as_ref().unwrap());
    assert!(escaped.contains("&lt;/script&gt;"));
    assert!(!escaped.contains("<img onerror"));
}
#[test]
fn all_theme_exports_have_complete_semantic_roles() {
    for t in grove_core::app_theme::CATALOG {
        let s = contract::export(t.id).unwrap();
        assert_eq!(s.colors.len(), 13);
        assert_eq!(s.theme, t.id);
        assert!(s.colors.values().all(|c| c.starts_with("rgba(")));
    }
}

#[cfg(all(target_os = "macos", grove_reference_capture))]
#[test]
#[ignore = "real Metal/fonts capture; explicitly scheduled by reference verifier"]
fn capture() {
    use gpui::AppContext as _;
    use std::sync::Arc;
    let doc = read_document();
    let out = output();
    let author = std::env::var("GROVE_REFERENCE_CAPTURE_MODE").as_deref() == Ok("author");
    let (s, lock) = if author {
        (artifacts(&doc, &out), None)
    } else {
        let (s, lock) = locked_snapshot(&doc, &out)
            .expect("candidate capture requires a frozen immutable reference");
        (s, Some(lock))
    };
    for name in ["certificate.json", "native.json", "native.png"] {
        if out.join(name).exists() {
            fs_err::remove_file(out.join(name)).unwrap();
        }
    }
    assert_eq!(
        include_str!(env!("GROVE_REFERENCE_GENERATED")),
        generate::generate(&doc, &s).unwrap(),
        "compiled generated source differs; generate and compile it before capture"
    );
    assert_eq!(
        generated::LAYOUT_HASH,
        generate::layout_hash(&doc),
        "generated pilot is stale; generate and compile the actual output first"
    );
    let mut cx = gpui::HeadlessAppContext::with_platform(
        gpui_platform::current_platform(true).text_system(),
        Arc::new(crate::assets::Assets),
        gpui_platform::current_headless_renderer,
    );
    cx.update(|cx| {
        gpui_component::init(cx);
        crate::fonts::register(cx).unwrap();
        cx.set_reduce_motion(true);
        assert!(grove_core::theme::set_by_name(&doc.fixture.theme));
        crate::theme::sync_component_theme(cx);
        gpui_component::Theme::global_mut(cx).font_size = gpui::px(doc.fixture.rem);
    });
    let measurements: native::Measurements = Rc::new(RefCell::new(BTreeMap::new()));
    let observed = measurements.clone();
    let action_log = Rc::new(RefCell::new(Vec::<String>::new()));
    let window = cx
        .open_window(
            gpui::size(
                gpui::px(doc.fixture.viewport.width),
                gpui::px(doc.fixture.viewport.height),
            ),
            |_window, cx| {
                let measurements = measurements.clone();
                let rem = doc.fixture.rem;
                let log = action_log.clone();
                let focus = cx.focus_handle();
                _window.focus(&focus, cx);
                let probe = cx.new(|_| Probe {
                    rem,
                    measurements,
                    focus,
                    bindings: generated::Bindings::fixture(Rc::new(move |action, _, _| {
                        log.borrow_mut().push(action.to_owned())
                    })),
                });
                cx.new(|cx| {
                    gpui_component::Root::new(probe, _window, cx)
                        .bordered(false)
                        .bg(crate::theme::BG())
                })
            },
        )
        .unwrap();
    for _ in 0..4 {
        cx.advance_clock(std::time::Duration::from_millis(16));
        cx.run_until_parked();
        cx.update_window(window.into(), |_, window, cx| {
            assert_eq!(f32::from(window.rem_size()), doc.fixture.rem);
            assert_eq!(window.scale_factor(), doc.fixture.scale);
            window.simulate_next_frame(cx);
            let _ = window.draw(cx);
        })
        .unwrap();
    }
    cx.capture_screenshot(window.into())
        .unwrap()
        .save(out.join("native.png"))
        .unwrap();
    // Compare the observer adapter with the original vendor SharedString path
    // in the same real render environment, before pointer/focus transitions.
    let observed_image = cx.capture_screenshot(window.into()).unwrap();
    native::set_observation_enabled(false);
    cx.update_window(window.into(), |_, window, cx| {
        window.refresh();
        window.simulate_next_frame(cx);
        let _ = window.draw(cx);
    })
    .unwrap();
    let original_image = cx.capture_screenshot(window.into()).unwrap();
    assert_eq!(
        observed_image.as_raw(),
        original_image.as_raw(),
        "native observer must preserve the genuine no-observer recipe pixels"
    );
    original_image
        .save(out.join("interaction-original-recipe.png"))
        .unwrap();
    native::set_observation_enabled(true);
    cx.update_window(window.into(), |_, window, cx| {
        window.refresh();
        window.simulate_next_frame(cx);
        let _ = window.draw(cx);
    })
    .unwrap();
    let initial = Rc::new(RefCell::new(observed.borrow().clone()));
    let interaction = super::interaction::observe(
        &mut cx,
        window.into(),
        &doc,
        &s,
        &observed,
        &action_log,
        &out,
    );
    let mut record = native::record(&doc, &s, &initial);
    record["interaction"] = interaction;
    document::validate_measurement(&doc, &s, &record).unwrap();
    record["reference_context"] = lock
        .map(|l| l["context"].clone())
        .unwrap_or_else(|| record["context"].clone());
    write(
        out.join("native.json"),
        serde_json::to_vec_pretty(&record).unwrap(),
    );
    if author {
        freeze(&doc, &s, &record, &out);
    }
    assert!(!observed.borrow().is_empty());
    cx.update_window(window.into(), |_, window, _| window.remove_window())
        .unwrap();
    println!("GROVE_REFERENCE: native capture: {}", out.display());
}
#[cfg(all(target_os = "macos", grove_reference_capture))]
struct Probe {
    focus: gpui::FocusHandle,
    rem: f32,
    measurements: native::Measurements,
    bindings: generated::Bindings,
}
#[cfg(all(target_os = "macos", grove_reference_capture))]
impl gpui::Render for Probe {
    fn render(
        &mut self,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        use gpui::{ParentElement as _, Styled as _};
        window.set_rem_size(gpui::px(self.rem));
        gpui::div()
            .id("reference-native-window")
            .track_focus(&self.focus)
            .on_key_down(crate::views::shell::traverse_unhandled_tab)
            .size_full()
            .when(native::mutation().as_deref() == Some("inset"), |node| {
                node.relative().left(gpui::px(1.))
            })
            .font_family(if native::mutation().as_deref() == Some("font") {
                crate::fonts::MONO_FAMILY
            } else {
                crate::fonts::UI_FAMILY
            })
            .font_weight(gpui::FontWeight::NORMAL)
            .text_size(crate::views::rpx(crate::views::tokens::TEXT_BODY))
            .text_color(crate::theme::FG())
            .bg(crate::theme::BG())
            .child(native::with_measurements(&self.measurements, || {
                generated::render(&self.bindings, window, cx)
            }))
    }
}

#[test]
fn measured_baseline_rejects_semantic_drift_and_preserves_locked_artifacts() {
    let s = contract::export("grove-dark").unwrap();
    let mut d = document::pilot(&s);
    d.root.kind = document::Kind::Text;
    d.root.id = "message".into();
    d.root.text = Some("Example".into());
    d.root.text_role = Some("text_small".into());
    d.root.color = Some("foreground".into());
    d.root.children.clear();
    d.actions.clear();
    d.values.clear();
    let record = serde_json::json!({"schema_version":1,"reference_hash":contract::hash(serde_json::to_vec(&d).unwrap()),"contract_hash":s.contract_hash,"context":{"synthetic_unit_model":true},"nodes":{"message":{"kind":"text","bounds":{"x":0,"y":0,"width":48,"height":12},"text":{"source_content":"Example","color":"rgba(247,247,248,1)","content":"Example","font_family":s.ui_family,"font_size":12,"font_weight":400,"font_style":"normal","line_height":12,"bounds":{"x":0,"y":0,"width":48,"height":12},"baseline":10,"glyphs":[{"text":"Example","x":0,"y":0,"width":48,"height":12}],"lines":[{"text":"Example","width":48,"y":0,"height":12,"baseline":10}]},"paint":{"kind":"text","foreground_rgba":s.colors_rgba["foreground"],"background_rgba":s.colors_rgba["background"],"html_fill":[247,247,248],"html_background":[17,17,17],"font_asset_sha256":s.font_hashes["fonts/DMSans-Regular.ttf"]}}}});
    document::validate_measurement(&d, &s, &record).unwrap();
    for mutation in 0..4 {
        let mut bad = record.clone();
        match mutation {
            0 => {
                bad["nodes"].as_object_mut().unwrap().remove("message");
            }
            1 => bad["nodes"]["message"]["text"]["source_content"] = serde_json::json!("Wrong"),
            2 => bad["nodes"]["message"]["text"]["font_size"] = serde_json::json!(20),
            _ => {
                bad["nodes"]["message"]["paint"]["foreground_rgba"] =
                    serde_json::json!([1, 0, 0, 1]);
            }
        }
        assert!(
            html::render_measured(&d, &s, &bad).is_err(),
            "semantic mutation {mutation}"
        );
    }
    let temp = tempfile::tempdir().unwrap();
    freeze(&d, &s, &record, temp.path());
    check_artifacts(&d, &s, temp.path()).unwrap();
    let original = fs_err::read(temp.path().join("reference.html")).unwrap();
    write(temp.path().join("generated.rs"), "// direct edit");
    assert!(check_artifacts(&d, &s, temp.path())
        .unwrap_err()
        .contains("generated.rs"));
    assert_eq!(
        fs_err::read(temp.path().join("reference.html")).unwrap(),
        original,
        "read-only check must preserve baseline"
    );
    write(
        temp.path().join("generated.rs"),
        generate::generate(&d, &s).unwrap(),
    );
    check_artifacts(&d, &s, temp.path()).unwrap();
    assert!(
        std::panic::catch_unwind(|| freeze(&d, &s, &record, temp.path())).is_err(),
        "author cannot overwrite frozen baseline"
    );
    assert_eq!(
        fs_err::read(temp.path().join("reference.html")).unwrap(),
        original
    );
}

#[test]
fn native_unorm_encoding_does_not_double_round() {
    let source = 248_f32 / 255.;
    let blended = source * 0.5 + 1. * (1. - 0.5);
    assert_eq!(f64::from(blended), 0.9862744808197021);
    assert_eq!((blended * 255.).round_ties_even(), 252.);
    assert_eq!(
        super::native::composite([source, source, source, 0.5], [1., 1., 1., 1.]),
        [251; 3]
    );
}

#[test]
fn same_background_paint_elision_requires_raw_native_proof() {
    let bg = [50_f32 / 255., 56_f32 / 255., 68_f32 / 255., 1.];
    let fill = [bg[0], bg[1], bg[2], 0.5];
    let mut paint = serde_json::json!({"kind":"rounded_rect","fill_rgba":fill,"background_rgba":bg,"html_fill":[50,56,68],"html_background":[50,56,68],"radius":4});
    assert!(html::elides_opaque_equal_background(&paint));
    paint["fill_rgba"] = serde_json::json!([0., 0., 0., 1.]);
    assert!(
        !html::elides_opaque_equal_background(&paint),
        "forged equal projected colors must not hide a different native fill"
    );
    paint["fill_rgba"] = serde_json::json!(fill);
    paint["background_rgba"][3] = serde_json::json!(0.5);
    assert!(!html::elides_opaque_equal_background(&paint));
}
