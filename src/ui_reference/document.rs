use super::contract::{Snapshot, DESIGN_SYSTEM, SCHEMA_VERSION};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Document {
    pub schema_version: u32,
    pub design_system: String,
    pub contract_hash: String,
    pub owners: BTreeMap<String, String>,
    pub fixture: Fixture,
    pub actions: Vec<String>,
    #[serde(default)]
    pub values: BTreeMap<String, String>,
    pub root: Node,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Fixture {
    pub viewport: Viewport,
    pub theme: String,
    pub rem: f32,
    pub scale: f32,
    pub locale: String,
    pub motion: String,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Viewport {
    pub width: f32,
    pub height: f32,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Row,
    Column,
    Surface,
    Text,
    Icon,
    DenseAction,
    CheckoutRow,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Rest,
    Hover,
    Focus,
    Disabled,
    Loading,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Width {
    Fill,
    Auto,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TextPolicy {
    SingleLine,
    Truncate,
    Wrap,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Layout {
    pub width: Width,
    pub gap: String,
    pub padding: String,
    pub text_policy: TextPolicy,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Node {
    pub id: String,
    pub kind: Kind,
    pub layout: Layout,
    pub state: State,
    pub text: Option<String>,
    #[serde(default)]
    pub text_binding: Option<String>,
    pub text_role: Option<String>,
    pub color: Option<String>,
    pub icon: Option<String>,
    pub action: Option<String>,
    pub children: Vec<Node>,
}
impl Node {
    pub fn source<'a>(&'a self, values: &'a BTreeMap<String, String>) -> Option<&'a str> {
        self.text.as_deref().or_else(|| {
            self.text_binding
                .as_ref()
                .and_then(|key| values.get(key).map(String::as_str))
        })
    }

    pub fn walk(&self, callback: &mut impl FnMut(&Node)) {
        callback(self);
        for child in &self.children {
            child.walk(callback);
        }
    }
}
fn validate_node(
    n: &Node,
    depth: usize,
    ids: &mut BTreeSet<String>,
    actions: &BTreeSet<&str>,
    values: &BTreeMap<String, String>,
    s: &Snapshot,
) -> Result<(), String> {
    if depth > 16 || ids.len() >= 200 || !identifier(&n.id) || !ids.insert(n.id.clone()) {
        return Err(format!(
            "{}: invalid/duplicate node ID or document limit",
            n.id
        ));
    }
    for token in [&n.layout.gap, &n.layout.padding] {
        if !s.tokens.contains_key(token) {
            return Err(format!("{}: unsupported layout token {token:?}", n.id));
        }
    }
    if !["space_sm", "space_lg", "space_2xl", "space_3xl"].contains(&n.layout.gap.as_str())
        || !["space_sm", "space_lg", "space_2xl", "space_3xl"].contains(&n.layout.padding.as_str())
    {
        return Err(format!("{}: layout requires a spacing token", n.id));
    }
    if n.layout.text_policy == TextPolicy::Wrap {
        return Err(format!(
            "{}: wrapping is unsupported until native/browser line metrics are certified",
            n.id
        ));
    }
    if n.kind == Kind::DenseAction && n.state == State::Loading && n.icon.is_none() {
        return Err(format!("{}: iconless loading is unsupported until its introduced native slot is separately certified",n.id));
    }
    if matches!(n.state, State::Hover | State::Focus) {
        return Err(format!("{}: transient hover/focus must be exercised through actual native input; static claimed states are unsupported",n.id));
    }
    if n.kind != Kind::DenseAction && n.state != State::Rest {
        return Err(format!("{}: state only supported on dense_action", n.id));
    }
    let text_bearing = matches!(n.kind, Kind::Text | Kind::DenseAction | Kind::CheckoutRow);
    if if text_bearing {
        !(n.text.is_some() ^ n.text_binding.is_some())
    } else {
        n.text.is_some() || n.text_binding.is_some()
    } {
        return Err(format!(
            "{}: text is required only on text/action/checkout_row",
            n.id
        ));
    }
    if n.text_binding
        .as_ref()
        .is_some_and(|key| !values.contains_key(key))
    {
        return Err(format!("{}: unknown value binding", n.id));
    }
    if let Some(t) = n.source(values) {
        let asset = crate::assets::Assets::get("fonts/DMSans-Regular.ttf")
            .ok_or("bundled UI font missing")?;
        let face = rustybuzz::Face::from_slice(&asset.data, 0).ok_or("bundled UI font invalid")?;
        if t.chars().any(|ch| face.glyph_index(ch).is_none()) {
            return Err(format!(
                "{}: unsupported text requires an unrecorded fallback font",
                n.id
            ));
        }
        if t.is_empty()
            || t.len() > 4096
            || t.contains(['\n', '\r', '\t'])
            || t.chars().any(char::is_control)
        {
            return Err(format!("{}: unsupported single-line text", n.id));
        }
    }
    if let Some(role) = &n.text_role {
        if !["text_small", "text_body", "text_title"].contains(&role.as_str()) {
            return Err(format!("{}: unsupported chrome typography {role}", n.id));
        }
    }
    if n.kind == Kind::Text && (n.text_role.is_none() || n.color.is_none()) {
        return Err(format!("{}: text role and color must be explicit", n.id));
    }
    if n.color.as_ref().is_some_and(|c| {
        ![
            "foreground",
            "supporting",
            "error",
            "success",
            "background",
            "rail",
            "hover",
            "border",
            "field",
        ]
        .contains(&c.as_str())
    }) {
        return Err(format!("{}: unknown color role", n.id));
    }
    if n.icon.as_ref().is_some_and(|i| !s.icons.contains_key(i)) {
        return Err(format!("{}: unsupported icon", n.id));
    }
    if n.kind == Kind::Icon && n.icon.is_none() {
        return Err(format!("{}: icon identity required", n.id));
    }
    if n.kind == Kind::DenseAction
        && n.action
            .as_ref()
            .is_none_or(|a| !actions.contains(a.as_str()))
    {
        return Err(format!("{}: missing declared action binding", n.id));
    }
    if n.kind != Kind::DenseAction && n.action.is_some() {
        return Err(format!("{}: action belongs to dense_action", n.id));
    }
    if matches!(n.kind, Kind::Text | Kind::Icon | Kind::DenseAction) && !n.children.is_empty() {
        return Err(format!("{}: leaf node cannot contain children", n.id));
    }
    if n.kind == Kind::CheckoutRow
        && (n.icon.is_none() || n.children.len() != 1 || n.children[0].kind != Kind::DenseAction)
    {
        return Err(format!(
            "{}: checkout_row requires one fixed trailing dense_action and a leading icon",
            n.id
        ));
    }
    let default_leaf = n.layout.gap == "space_lg" && n.layout.padding == "space_lg";
    let supported = match n.kind {
        Kind::Row | Kind::Column | Kind::Surface => {
            n.layout.width == Width::Fill
                && n.text_role.is_none()
                && n.color.is_none()
                && n.icon.is_none()
                && n.layout.text_policy == TextPolicy::SingleLine
        }
        Kind::DenseAction => {
            n.text_role.is_none()
                && n.color.is_none()
                && default_leaf
                && n.layout.width == Width::Auto
                && n.layout.text_policy == TextPolicy::SingleLine
        }
        Kind::CheckoutRow => {
            n.text_role.as_deref() == s.checkout_row["text_token"].as_str()
                && n.color.as_deref() == Some("foreground")
                && n.layout.padding == s.checkout_row["padding_token"].as_str().unwrap()
                && n.layout.gap == s.checkout_row["gap_token"].as_str().unwrap()
                && n.layout.width == Width::Fill
                && n.layout.text_policy == TextPolicy::Truncate
        }
        Kind::Icon => {
            n.text_role.is_none()
                && n.color.is_none()
                && default_leaf
                && n.layout.width == Width::Auto
                && n.layout.text_policy == TextPolicy::SingleLine
        }
        Kind::Text => {
            n.icon.is_none() && default_leaf && n.layout.text_policy == TextPolicy::SingleLine
        }
    };
    if !supported {
        return Err(format!(
            "{}: unsupported property combination; no ignored or approximated overrides",
            n.id
        ));
    }
    for child in &n.children {
        validate_node(child, depth + 1, ids, actions, values, s)?;
    }
    Ok(())
}

pub fn validate(doc: &Document, snapshot: &Snapshot) -> Result<(), String> {
    if doc.schema_version != SCHEMA_VERSION || doc.design_system != DESIGN_SYSTEM {
        return Err("unsupported reference schema or design system".into());
    }
    let effective_owners = super::contract::effective_owners(snapshot);
    if doc.contract_hash != snapshot.contract_hash || doc.owners != effective_owners {
        let changed = effective_owners
            .iter()
            .filter(|(k, v)| doc.owners.get(*k) != Some(*v))
            .map(|(k, _)| k.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!("stale design-system contract; re-render and review reference; changed owners: {changed}"));
    }
    let f = &doc.fixture;
    if f.theme != snapshot.theme
        || ![16., 20., 24.].contains(&f.rem)
        || f.scale != 2.
        || f.locale != "en-US"
        || f.motion != "reduced"
        || !f.viewport.width.is_finite()
        || !f.viewport.height.is_finite()
        || !(240. ..=1600.).contains(&f.viewport.width)
        || !(80. ..=1200.).contains(&f.viewport.height)
    {
        return Err(
            "unsupported fixture context (theme, rem, scale=2, en-US, reduced motion, or viewport)"
                .into(),
        );
    }
    let mut actions = BTreeSet::new();
    for a in &doc.actions {
        if !action_identifier(a) || !actions.insert(a.as_str()) {
            return Err(format!("invalid/duplicate action {a:?}"));
        }
    }
    for key in doc.values.keys() {
        if !action_identifier(key) || actions.contains(key.as_str()) {
            return Err(format!("invalid/conflicting value binding {key:?}"));
        }
    }
    let mut ids = BTreeSet::new();
    validate_node(&doc.root, 0, &mut ids, &actions, &doc.values, snapshot)?;
    let mut used_values = BTreeSet::new();
    let mut used_actions = BTreeSet::new();
    doc.root.walk(&mut |n| {
        if let Some(v) = &n.text_binding {
            used_values.insert(v.clone());
        }
        if let Some(a) = &n.action {
            used_actions.insert(a.clone());
        }
    });
    if used_values.iter().ne(doc.values.keys())
        || used_actions
            .iter()
            .map(String::as_str)
            .ne(actions.iter().copied())
    {
        return Err("unused value/action declarations are unsupported".into());
    }
    if !used_actions.is_empty() {
        let mut states = [false; 3];
        doc.root.walk(&mut |node| {
            if node.kind == Kind::DenseAction {
                match node.state {
                    State::Rest => states[0] = true,
                    State::Disabled => states[1] = true,
                    State::Loading => states[2] = true,
                    _ => {}
                }
            }
        });
        if states != [true; 3] {
            return Err(
                "dense_action certification requires explicit rest, disabled and loading fixtures"
                    .into(),
            );
        }
    }
    let authored = ids.clone();
    let mut error = None;
    doc.root.walk(&mut |n| {
        if matches!(n.kind, Kind::DenseAction | Kind::CheckoutRow) {
            for suffix in ["label", "icon"] {
                let id = format!("{}-{suffix}", n.id);
                if authored.contains(&id) || !ids.insert(id.clone()) {
                    error = Some(format!("reserved generated node ID {id}"));
                }
            }
        }
    });
    error.map_or(Ok(()), Err)
}
pub fn action_identifier(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 80
        && s.as_bytes()[0].is_ascii_lowercase()
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        && ![
            "self", "super", "crate", "fn", "type", "move", "ref", "match", "loop", "async",
            "await", "dyn", "impl", "trait", "mod", "pub", "use", "const", "static", "let", "mut",
            "in", "if", "else", "for", "while", "return", "break", "continue", "where", "as",
            "extern", "unsafe", "struct", "enum", "union", "true", "false", "abstract", "become",
            "box", "do", "final", "macro", "override", "priv", "typeof", "unsized", "virtual",
            "yield", "try", "gen",
        ]
        .contains(&s)
}
pub fn identifier(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 80
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

pub fn pilot(snapshot: &Snapshot) -> Document {
    let layout = Layout {
        width: Width::Fill,
        gap: "space_lg".into(),
        padding: "space_lg".into(),
        text_policy: TextPolicy::SingleLine,
    };
    let mut root = Node {
        id: "pilot".into(),
        kind: Kind::Column,
        layout: layout.clone(),
        state: State::Rest,
        text: None,
        text_binding: None,
        text_role: None,
        color: None,
        icon: None,
        action: None,
        children: vec![],
    };
    for (id, state, _label) in [
        ("rest", State::Rest, "Run"),
        ("disabled", State::Disabled, "Run"),
        ("loading", State::Loading, "Run"),
    ] {
        let action = Node {
            id: format!("action-{id}"),
            kind: Kind::DenseAction,
            layout: Layout {
                width: Width::Auto,
                ..layout.clone()
            },
            state,
            text: None,
            text_binding: Some("run_label".into()),
            text_role: None,
            color: None,
            icon: Some("play".into()),
            action: Some("run".into()),
            children: vec![],
        };
        root.children.push(Node {
            id: format!("row-{id}"),
            kind: Kind::CheckoutRow,
            layout: Layout {
                text_policy: TextPolicy::Truncate,
                padding: snapshot.checkout_row["padding_token"]
                    .as_str()
                    .unwrap()
                    .into(),
                gap: snapshot.checkout_row["gap_token"].as_str().unwrap().into(),
                ..layout.clone()
            },
            state: State::Rest,
            text_binding: None,
            text: Some(if id == "rest" {
                "Build checkout".into()
            } else {
                "A long checkout label that must yield to the fixed trailing action".into()
            }),
            text_role: Some(snapshot.checkout_row["text_token"].as_str().unwrap().into()),
            color: Some("foreground".into()),
            icon: Some("folder".into()),
            action: None,
            children: vec![action],
        });
    }
    Document {
        schema_version: SCHEMA_VERSION,
        design_system: DESIGN_SYSTEM.into(),
        contract_hash: snapshot.contract_hash.clone(),
        owners: super::contract::effective_owners(snapshot),
        fixture: Fixture {
            viewport: Viewport {
                width: 420.,
                height: 180.,
            },
            theme: snapshot.theme.clone(),
            rem: 16.,
            scale: 2.,
            locale: "en-US".into(),
            motion: "reduced".into(),
        },
        actions: vec!["run".into()],
        values: BTreeMap::from([("run_label".into(), "Run".into())]),
        root,
    }
}

/// Independent authored semantics survive the measured-layout fallback.
/// A missing/wrong native node must never become a new reference baseline.
struct Expected {
    kind: &'static str,
    text: Option<String>,
    size: Option<f32>,
    color: Option<[f32; 4]>,
    icon: Option<String>,
    truncate: bool,
    radius: Option<f32>,
    fill: Option<[f32; 4]>,
}

pub fn validate_measurement(
    doc: &Document,
    s: &Snapshot,
    record: &serde_json::Value,
) -> Result<(), String> {
    use super::native::MeasuredNode;
    validate(doc, s)?;
    if record["reference_hash"] != super::contract::hash(serde_json::to_vec(doc).unwrap())
        || record["contract_hash"] != s.contract_hash
    {
        return Err("measured source identity mismatch".into());
    }
    let nodes: BTreeMap<String, MeasuredNode> =
        serde_json::from_value(record["nodes"].clone()).map_err(|e| e.to_string())?;
    let ratio = doc.fixture.rem / s.rem_base;
    let mut expected = BTreeMap::new();
    doc.root.walk(&mut |n| {
        let kind = match n.kind {
            Kind::Row => "row",
            Kind::Column => "column",
            Kind::Surface => "surface",
            Kind::Text => "text",
            Kind::Icon => "icon",
            Kind::DenseAction => "dense_action",
            Kind::CheckoutRow => "checkout_row",
        };
        let is_text = n.kind == Kind::Text;
        let fill = if n.kind == Kind::DenseAction {
            Some(
                s.colors_rgba[match n.state {
                    State::Disabled => "rail_disabled",
                    State::Loading => "rail_loading",
                    State::Hover | State::Focus => "hover",
                    State::Rest => "rail",
                }],
            )
        } else if n.kind == Kind::Surface {
            Some(s.colors_rgba["rail"])
        } else {
            None
        };
        expected.insert(
            n.id.clone(),
            Expected {
                kind,
                text: if is_text {
                    n.source(&doc.values).map(str::to_owned)
                } else {
                    None
                },
                size: if is_text {
                    Some(s.tokens[n.text_role.as_ref().unwrap()] * ratio)
                } else {
                    None
                },
                color: if is_text {
                    Some(s.colors_rgba[n.color.as_ref().unwrap()])
                } else {
                    None
                },
                icon: if n.kind == Kind::Icon {
                    n.icon.clone()
                } else {
                    None
                },
                truncate: false,
                radius: if n.kind == Kind::Surface {
                    Some(0.)
                } else {
                    (n.kind == Kind::DenseAction)
                        .then_some(s.dense_action["radius"].as_f64().unwrap() as f32 * ratio)
                },
                fill,
            },
        );
        if matches!(n.kind, Kind::DenseAction | Kind::CheckoutRow) {
            let color = s.colors_rgba[if n.kind == Kind::CheckoutRow {
                "foreground"
            } else {
                match n.state {
                    State::Disabled => "supporting_disabled",
                    State::Loading => "supporting_loading",
                    _ => "supporting",
                }
            }];
            expected.insert(
                format!("{}-label", n.id),
                Expected {
                    kind: "text",
                    text: n.source(&doc.values).map(str::to_owned),
                    size: Some(if n.kind == Kind::CheckoutRow {
                        s.checkout_row["text_size"].as_f64().unwrap() as f32 * ratio
                    } else {
                        s.dense_action["text_size"].as_f64().unwrap() as f32 * ratio
                    }),
                    color: Some(color),
                    icon: None,
                    truncate: n.kind == Kind::CheckoutRow,
                    radius: None,
                    fill: None,
                },
            );
            if let Some(icon) = &n.icon {
                expected.insert(
                    format!("{}-icon", n.id),
                    Expected {
                        kind: "icon",
                        text: None,
                        size: None,
                        color: Some(if n.kind == Kind::CheckoutRow {
                            s.colors_rgba["supporting"]
                        } else {
                            color
                        }),
                        icon: Some(
                            if n.kind == Kind::DenseAction && n.state == State::Loading {
                                s.dense_action["loading_icon"].as_str().unwrap().into()
                            } else {
                                icon.clone()
                            },
                        ),
                        truncate: false,
                        radius: None,
                        fill: None,
                    },
                );
            }
        }
    });
    if nodes.keys().ne(expected.keys()) {
        return Err("measured node set differs from authored and reserved nodes".into());
    }
    for (id, e) in expected {
        let n = &nodes[&id];
        if n.kind != e.kind {
            return Err(format!("{id}: measured kind differs from authored kind"));
        }
        let paint = n.paint.as_ref();
        if let Some(source) = e.text {
            let t = n
                .text
                .as_ref()
                .ok_or_else(|| format!("{id}: missing measured text"))?;
            let visible_valid = t.content == source
                || (e.truncate
                    && t.content
                        .strip_suffix('…')
                        .is_some_and(|prefix| !prefix.is_empty() && source.starts_with(prefix)));
            if t.source_content != source
                || !visible_valid
                || t.font_family != s.ui_family
                || t.font_weight != 400
                || t.font_style != "normal"
                || t.font_size != e.size.unwrap()
                || t.line_height
                    != if e.truncate {
                        ((t.font_size * s.native_line_height_multiplier * doc.fixture.scale).abs()
                            - 0.5)
                            .ceil()
                            .copysign(t.font_size * s.native_line_height_multiplier)
                            / doc.fixture.scale
                    } else {
                        t.font_size
                    }
                || t.glyphs.iter().map(|g| g.text.as_str()).collect::<String>() != t.content
            {
                return Err(format!(
                    "{id}: measured text/font/truncation violates authored content or type role"
                ));
            }
            let p = paint.ok_or_else(|| format!("{id}: missing text paint"))?;
            if p["font_asset_sha256"] != s.font_hashes["fonts/DMSans-Regular.ttf"] {
                return Err(format!("{id}: unsupported effective font asset"));
            }
        } else if n.text.is_some() {
            return Err(format!("{id}: unexpected measured text"));
        }
        if let Some(color) = e.color {
            let p = paint.ok_or_else(|| format!("{id}: missing foreground metadata"))?;
            if serde_json::from_value::<[f32; 4]>(p["foreground_rgba"].clone())
                .map_err(|e| e.to_string())?
                != color
            {
                return Err(format!(
                    "{id}: actual native foreground differs from design-system role"
                ));
            }
        }
        if let Some(icon) = e.icon {
            let p = paint.ok_or_else(|| format!("{id}: missing actual icon asset"))?;
            if p["asset_svg"] != s.icons[&icon]
                || p["asset_sha256"] != super::contract::hash(s.icons[&icon].as_bytes())
            {
                return Err(format!(
                    "{id}: actual native icon differs from catalog identity"
                ));
            }
        }
        if let Some(radius) = e.radius {
            let p = paint.ok_or_else(|| format!("{id}: missing native button paint"))?;
            if p["radius"].as_f64() != Some(f64::from(radius))
                || serde_json::from_value::<[f32; 4]>(p["fill_rgba"].clone())
                    .map_err(|e| e.to_string())?
                    != e.fill.unwrap()
            {
                return Err(format!(
                    "{id}: actual native button recipe differs from exported design system"
                ));
            }
            if e.kind == "dense_action"
                && n.bounds.height != s.dense_action["height"].as_f64().unwrap() as f32 * ratio
            {
                return Err(format!("{id}: wrong native control height"));
            }
        }
    }
    let r = &s.checkout_row;
    let mut row_error = None;
    doc.root.walk(&mut|n|{
        if n.kind!=Kind::CheckoutRow{return;}
        let row=&nodes[&n.id].bounds;let label=&nodes[&format!("{}-label",n.id)].bounds;let icon=&nodes[&format!("{}-icon",n.id)].bounds;let action=&nodes[&n.children[0].id].bounds;
        let number=|key:&str|r[key].as_f64().unwrap()as f32*ratio;
        if row.height!=number("height") || label.x-row.x!=number("padding_x")+number("icon_slot")+number("gap") || icon.x-row.x!=number("padding_x") || icon.width!=number("icon_size") || action.x+action.width>row.x+row.width-number("padding_x")+0.00001 {
            row_error=Some(format!("{}: native row violates registered frame, leading-slot or trailing-action geometry",n.id));
        }
    });
    if let Some(error) = row_error {
        return Err(error);
    }
    Ok(())
}
