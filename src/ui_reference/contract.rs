use std::collections::BTreeMap;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::{assets::Assets, fonts, icons, theme as c, views::tokens::*};

pub const SCHEMA_VERSION: u32 = 1;
pub const GENERATOR_VERSION: &str = "grove-reference-1";
pub const DESIGN_SYSTEM: &str = "grove";

pub fn hash(bytes: impl AsRef<[u8]>) -> String {
    format!("{:x}", Sha256::digest(bytes.as_ref()))
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FontFace {
    pub path: String,
    pub family: String,
    pub weight: u16,
    pub style: String,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub schema_version: u32,
    pub rem_base: f32,
    pub design_system: String,
    pub generator_version: String,
    pub source_revision: String,
    pub contract_hash: String,
    pub owners: BTreeMap<String, String>,
    pub tokens: BTreeMap<String, f32>,
    pub colors: BTreeMap<String, String>,
    pub colors_rgba: BTreeMap<String, [f32; 4]>,
    pub theme: String,
    pub ui_family: String,
    pub mono_family: String,
    pub font_hashes: BTreeMap<String, String>,
    pub font_faces: Vec<FontFace>,
    pub font_data: BTreeMap<String, String>,
    pub icons: BTreeMap<String, String>,
    pub dense_action: serde_json::Value,
    pub checkout_row: serde_json::Value,
    pub native_line_height_multiplier: f32,
}

pub fn export(theme: &str) -> Result<Snapshot, String> {
    if grove_core::app_theme::by_id(theme).is_none() {
        return Err(format!("unknown application theme {theme:?}"));
    }
    let _lock = crate::theme::ACTIVE_THEME_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let previous = grove_core::theme::current().name.clone();
    if !grove_core::theme::set_by_name(theme) {
        return Err(format!("cannot resolve theme {theme:?}"));
    }
    let roles = [
        ("background", c::BG()),
        ("rail", c::BG_RAIL()),
        ("hover", c::BG_HOVER()),
        ("foreground", c::FG()),
        ("supporting", c::FG_DIM()),
        ("border", c::BORDER()),
        ("field", c::FIELD_FILL()),
        ("error", c::RED()),
        ("success", c::GREEN()),
        (
            "rail_disabled",
            c::alpha(
                c::BG_RAIL(),
                crate::views::components::DENSE_ACTION_RECIPE.disabled_opacity,
            ),
        ),
        (
            "supporting_disabled",
            c::alpha(
                c::FG_DIM(),
                crate::views::components::DENSE_ACTION_RECIPE.disabled_opacity,
            ),
        ),
        (
            "rail_loading",
            c::alpha(
                c::BG_RAIL(),
                crate::views::components::DENSE_ACTION_RECIPE.loading_opacity,
            ),
        ),
        (
            "supporting_loading",
            c::alpha(
                c::FG_DIM(),
                crate::views::components::DENSE_ACTION_RECIPE.loading_opacity,
            ),
        ),
    ];
    let colors_rgba: BTreeMap<_, _> = roles
        .into_iter()
        .map(|(name, color)| {
            let c: gpui::Rgba = color.into();
            (name.to_owned(), [c.r, c.g, c.b, c.a])
        })
        .collect();
    let colors = colors_rgba
        .iter()
        .map(|(name, c)| {
            (
                name.clone(),
                format!(
                    "rgba({:.6},{:.6},{:.6},{:.6})",
                    c[0] * 255.,
                    c[1] * 255.,
                    c[2] * 255.,
                    c[3]
                ),
            )
        })
        .collect();
    grove_core::theme::set_by_name(&previous);
    let owners = [
        ("DESIGN.md", include_bytes!("../../DESIGN.md").as_slice()),
        (
            "src/views/tokens.rs",
            include_bytes!("../views/tokens.rs").as_slice(),
        ),
        (
            "src/views/components.rs",
            include_bytes!("../views/components.rs").as_slice(),
        ),
        ("src/theme.rs", include_bytes!("../theme.rs").as_slice()),
        ("src/fonts.rs", include_bytes!("../fonts.rs").as_slice()),
        ("src/icons.rs", include_bytes!("../icons.rs").as_slice()),
        (
            "reference/generate",
            include_bytes!("generate.rs").as_slice(),
        ),
        ("reference/html", include_bytes!("html.rs").as_slice()),
        ("reference/native", include_bytes!("native.rs").as_slice()),
        ("reference/compose", include_bytes!("compose.rs").as_slice()),
        (
            "reference/production-guard",
            include_bytes!("production_guard.rs").as_slice(),
        ),
        (
            "reference/contract",
            include_bytes!("contract.rs").as_slice(),
        ),
        ("build.rs", include_bytes!("../../build.rs").as_slice()),
        ("reference/runtime", include_bytes!("runtime.rs").as_slice()),
        (
            "reference/generated-recipe",
            include_bytes!("generated_dense_action.rs").as_slice(),
        ),
        ("reference/schema", include_bytes!("document.rs").as_slice()),
        (
            "vendor/button",
            include_bytes!("../../vendor/gpui-component/ui/src/button/button.rs").as_slice(),
        ),
        (
            "vendor/styled",
            include_bytes!("../../vendor/gpui-component/ui/src/styled.rs").as_slice(),
        ),
        (
            "vendor/icon",
            include_bytes!("../../vendor/gpui-component/ui/src/icon.rs").as_slice(),
        ),
        (
            "vendor/button_icon",
            include_bytes!("../../vendor/gpui-component/ui/src/button/button_icon.rs").as_slice(),
        ),
        (
            "vendor/spinner",
            include_bytes!("../../vendor/gpui-component/ui/src/spinner.rs").as_slice(),
        ),
        (
            "workspace-manifest",
            include_bytes!("../../Cargo.toml").as_slice(),
        ),
        (
            "rust-toolchain",
            include_bytes!("../../rust-toolchain.toml").as_slice(),
        ),
        (
            "native-engine",
            include_bytes!("../../Cargo.lock").as_slice(),
        ),
    ]
    .into_iter()
    .map(|(path, data)| (path.to_owned(), hash(data)))
    .collect();
    let tokens = [
        ("space_sm", SPACE_SM),
        ("space_lg", SPACE_LG),
        ("space_2xl", SPACE_2XL),
        ("space_3xl", SPACE_3XL),
        ("text_small", TEXT_SMALL),
        ("text_body", TEXT_BODY),
        ("text_title", TEXT_TITLE),
        ("icon_sm", ICON_SM),
        ("control_h", CONTROL_H),
        ("radius_control", RADIUS_CONTROL),
        ("radius_panel", RADIUS_PANEL),
        ("row_px", ROW_PX),
        ("row_py", ROW_PY),
        ("row_min_h", ROW_MIN_H),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v))
    .collect();
    let mut font_hashes = BTreeMap::new();
    let mut font_data = BTreeMap::new();
    let mut font_faces = Vec::new();
    for face in fonts::FONT_FACES {
        let path = face.path;
        let asset = Assets::get(path).ok_or_else(|| format!("missing bundled font {path}"))?;
        let sha256 = hash(&asset.data);
        font_faces.push(FontFace {
            path: path.to_owned(),
            family: face.family.to_owned(),
            weight: face.weight,
            style: face.style.to_owned(),
            sha256: sha256.clone(),
        });
        font_hashes.insert(path.to_owned(), sha256);
        font_data.insert(path.to_owned(), STANDARD.encode(&asset.data));
    }
    // This is the certified pilot's allowlist, not a duplicate icon implementation.
    let icons = ["plus", "play", "folder", "check", "reset", "status-working"]
        .into_iter()
        .map(|name| {
            (
                name.to_owned(),
                icons::svg_for_path(&format!("icons/{name}.svg")).unwrap(),
            )
        })
        .collect();
    let mut result = Snapshot {
        schema_version: SCHEMA_VERSION,
        rem_base: crate::zoom::REM_BASE,
        design_system: DESIGN_SYSTEM.to_owned(),
        generator_version: GENERATOR_VERSION.to_owned(),
        source_revision: std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .output()
            .map_err(|e| e.to_string())
            .and_then(|o| {
                if !o.status.success() {
                    return Err("git revision lookup failed".to_owned());
                }
                let value = String::from_utf8(o.stdout).map_err(|e| e.to_string())?;
                if value.trim().is_empty() {
                    return Err("empty git revision".to_owned());
                }
                Ok(value)
            })?
            .trim()
            .to_owned(),
        contract_hash: String::new(),
        owners,
        tokens,
        colors,
        colors_rgba,
        theme: theme.to_owned(),
        ui_family: fonts::UI_FAMILY.to_owned(),
        mono_family: fonts::MONO_FAMILY.to_owned(),
        font_hashes,
        font_faces,
        font_data,
        icons,
        checkout_row: serde_json::to_value(crate::views::components::CHECKOUT_REGISTER_RECIPE)
            .unwrap(),
        native_line_height_multiplier: gpui::TextStyle::default()
            .line_height
            .to_pixels(gpui::px(1.).into(), gpui::px(crate::zoom::REM_BASE))
            .into(),
        dense_action: serde_json::to_value(crate::views::components::DENSE_ACTION_RECIPE)
            .map_err(|e| e.to_string())?,
    };
    result.contract_hash = fingerprint(&result);
    Ok(result)
}

pub fn fingerprint(snapshot: &Snapshot) -> String {
    let mut payload = serde_json::to_value(snapshot).unwrap();
    let obj = payload.as_object_mut().unwrap();
    obj.remove("contract_hash");
    obj.remove("source_revision");
    hash(serde_json::to_vec(&payload).unwrap())
}

/// Raw owners plus fingerprints of their captured effective native outputs.
/// This keeps stale diagnostics precise even when effective inputs change
/// without a source-file edit (palette selection or injected exporter tests).
pub fn effective_owners(snapshot: &Snapshot) -> BTreeMap<String, String> {
    let mut owners = snapshot.owners.clone();
    for (name, value) in [
        (
            "resolved/src/views/tokens.rs",
            serde_json::to_value(&snapshot.tokens).unwrap(),
        ),
        (
            "resolved/src/theme.rs",
            serde_json::json!((&snapshot.theme, &snapshot.colors, &snapshot.colors_rgba)),
        ),
        (
            "resolved/src/fonts.rs",
            serde_json::json!((
                &snapshot.font_hashes,
                &snapshot.font_faces,
                &snapshot.font_data
            )),
        ),
        (
            "resolved/src/views/components.rs",
            serde_json::json!((&snapshot.dense_action, &snapshot.checkout_row)),
        ),
    ] {
        owners.insert(name.into(), hash(serde_json::to_vec(&value).unwrap()));
    }
    owners
}
