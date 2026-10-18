//! Restricted MCP elicitation forms. Unsupported constraints are rejected, never ignored.
use serde_json::{Map, Value};

#[derive(Clone, Debug, PartialEq)]
pub struct ElicitationOption {
    pub value: String,
    pub title: String,
}
#[derive(Clone, Debug, PartialEq)]
pub enum ElicitationKind {
    String {
        format: Option<String>,
        min_length: Option<usize>,
        max_length: Option<usize>,
    },
    Number {
        integer: bool,
        minimum: Option<f64>,
        maximum: Option<f64>,
    },
    Boolean,
    Select {
        options: Vec<ElicitationOption>,
    },
    MultiSelect {
        options: Vec<ElicitationOption>,
        min_items: Option<usize>,
        max_items: Option<usize>,
    },
}
#[derive(Clone, Debug, PartialEq)]
pub struct ElicitationField {
    pub name: String,
    pub title: String,
    pub description: Option<String>,
    pub required: bool,
    pub kind: ElicitationKind,
    pub default: Option<Value>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct ElicitationForm {
    pub fields: Vec<ElicitationField>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct ElicitationValidationError {
    pub field: String,
    pub message: String,
}
#[derive(Clone, Debug, PartialEq)]
pub enum ElicitationMode {
    Form(ElicitationForm),
    Url { url: String, elicitation_id: String },
}
#[derive(Clone, Debug, PartialEq)]
pub struct ElicitationRequest {
    pub id: Value,
    pub turn_id: Option<String>,
    pub server_name: String,
    pub message: String,
    pub mode: ElicitationMode,
}

pub fn is_form_mode(mode: &str) -> bool {
    matches!(mode, "form" | "openaiForm" | "openai/form")
}
impl ElicitationRequest {
    pub fn parse_codex(id: Value, params: &Value) -> Result<Self, String> {
        Self::parse(id, params)
    }
    pub fn parse_claude(id: Value, request: &Value) -> Result<Self, String> {
        Self::parse(id, request)
    }
    fn parse(id: Value, p: &Value) -> Result<Self, String> {
        let mode = p.get("mode").and_then(Value::as_str).unwrap_or("form");
        let mode = if is_form_mode(mode) {
            ElicitationMode::Form(ElicitationForm::parse(
                p.get("requestedSchema")
                    .or_else(|| p.get("requested_schema"))
                    .ok_or("Missing requested schema")?,
            )?)
        } else if mode == "url" {
            let url = p
                .get("url")
                .and_then(Value::as_str)
                .ok_or("Missing URL")?
                .to_owned();
            validate_elicitation_url(&url)?;
            ElicitationMode::Url {
                url,
                elicitation_id: p
                    .get("elicitationId")
                    .or_else(|| p.get("elicitation_id"))
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .ok_or("Missing elicitation ID")?
                    .to_owned(),
            }
        } else {
            return Err(format!(
                "Unsupported elicitation mode {mode}; decline this request"
            ));
        };
        Ok(Self {
            id,
            turn_id: p.get("turnId").and_then(Value::as_str).map(str::to_owned),
            mode,
            server_name: p
                .get("serverName")
                .or_else(|| p.get("server_name"))
                .or_else(|| p.get("mcp_server_name"))
                .and_then(Value::as_str)
                .unwrap_or("MCP server")
                .to_owned(),
            message: p
                .get("message")
                .and_then(Value::as_str)
                .ok_or("Missing elicitation message")?
                .to_owned(),
        })
    }
    pub fn validate_content(&self, content: &Value) -> Result<(), String> {
        match &self.mode {
            ElicitationMode::Form(form) => {
                let content = content
                    .as_object()
                    .ok_or("Form response must be an object")?;
                form.validate(content).map(|_| ()).map_err(|errors| {
                    errors
                        .into_iter()
                        .map(|e| format!("{}: {}", e.field, e.message))
                        .collect::<Vec<_>>()
                        .join("; ")
                })
            }
            ElicitationMode::Url { .. } if content.is_null() => Ok(()),
            ElicitationMode::Url { .. } => {
                Err("URL elicitation does not accept form content".into())
            }
        }
    }
}
fn keys(value: &Map<String, Value>, allowed: &[&str]) -> Result<(), String> {
    if let Some(key) = value.keys().find(|k| !allowed.contains(&k.as_str())) {
        return Err(format!(
            "Unsupported schema constraint {key}; ask the server for a supported flat form or URL flow"
        ));
    }
    Ok(())
}
fn text(v: &Value, key: &str) -> Result<Option<String>, String> {
    match v.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        _ => Err(format!("{key} must be text")),
    }
}
fn count(v: &Value, key: &str) -> Result<Option<usize>, String> {
    match v.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(n) => n
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .map(Some)
            .ok_or_else(|| format!("{key} must be a nonnegative integer")),
    }
}
fn bound(v: &Value, key: &str) -> Result<Option<f64>, String> {
    match v.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(n) => n
            .as_f64()
            .map(Some)
            .ok_or_else(|| format!("{key} must be numeric")),
    }
}
fn limits<T: PartialOrd>(min: Option<T>, max: Option<T>) -> Result<(), String> {
    if matches!((min,max),(Some(a),Some(b)) if a>b) {
        Err("Minimum exceeds maximum".into())
    } else {
        Ok(())
    }
}
fn options(v: &Value, titled: &str) -> Result<Vec<ElicitationOption>, String> {
    let result = if let Some(values) = v.get("enum") {
        let values = values.as_array().ok_or("enum must be an array")?;
        let names = v
            .get("enumNames")
            .filter(|n| !n.is_null())
            .map(|n| n.as_array().ok_or("enumNames must be an array"))
            .transpose()?;
        if names.is_some_and(|n| n.len() != values.len()) {
            return Err("enumNames must match enum length".into());
        }
        values
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let value = v.as_str().ok_or("Enum values must be strings")?.to_owned();
                let title = match names {
                    Some(n) => n[i]
                        .as_str()
                        .ok_or("enumNames must contain strings")?
                        .to_owned(),
                    None => value.clone(),
                };
                Ok(ElicitationOption { value, title })
            })
            .collect::<Result<Vec<_>, String>>()?
    } else {
        v.get(titled)
            .and_then(Value::as_array)
            .ok_or("Missing enum choices")?
            .iter()
            .map(|o| {
                keys(
                    o.as_object().ok_or("Choice must be an object")?,
                    &["const", "title"],
                )?;
                Ok(ElicitationOption {
                    value: text(o, "const")?.ok_or("Missing choice const")?,
                    title: text(o, "title")?.ok_or("Missing choice title")?,
                })
            })
            .collect::<Result<Vec<_>, String>>()?
    };
    if result.is_empty()
        || result
            .iter()
            .enumerate()
            .any(|(i, o)| result[..i].iter().any(|p| p.value == o.value))
    {
        return Err("Choices must be nonempty and unique".into());
    }
    Ok(result)
}
impl ElicitationForm {
    pub fn parse(schema: &Value) -> Result<Self, String> {
        let obj = schema.as_object().ok_or("Schema must be an object")?;
        keys(obj, &["$schema", "type", "properties", "required"])?;
        if schema.get("type").and_then(Value::as_str) != Some("object") {
            return Err("Only flat object forms are supported".into());
        }
        let props = schema
            .get("properties")
            .and_then(Value::as_object)
            .ok_or("Missing form properties")?;
        let required = match schema.get("required") {
            None | Some(Value::Null) => vec![],
            Some(v) => v
                .as_array()
                .ok_or("required must be an array")?
                .iter()
                .map(|v| v.as_str().ok_or("required entries must be strings"))
                .collect::<Result<Vec<_>, _>>()?,
        };
        if required.iter().any(|key| !props.contains_key(*key)) {
            return Err("Required field is absent from properties".into());
        }
        let mut fields = Vec::new();
        for (name, v) in props {
            // MCP form mode must not transport credentials. These explicit field
            // names are rejected; URL mode keeps such data outside the client.
            let normalized_name = name.to_ascii_lowercase().replace(['-', '_', ' '], "");
            if matches!(
                normalized_name.as_str(),
                "password"
                    | "passwd"
                    | "apikey"
                    | "accesstoken"
                    | "refreshtoken"
                    | "authorization"
                    | "privatekey"
                    | "clientsecret"
                    | "creditcard"
                    | "cardnumber"
                    | "cvv"
            ) {
                return Err(format!(
                    "Credential field {name} requires a URL elicitation flow"
                ));
            }

            let object = v.as_object().ok_or("Field schema must be an object")?;
            let mut allowed = vec!["type", "title", "description", "default"];
            let kind = match v.get("type").and_then(Value::as_str) {
                Some("string") if v.get("enum").is_some() || v.get("oneOf").is_some() => {
                    allowed.extend(["enum", "enumNames", "oneOf"]);
                    if v.get("enum").is_some() && v.get("oneOf").is_some() {
                        return Err("Ambiguous enum schema".into());
                    }
                    ElicitationKind::Select {
                        options: options(v, "oneOf")?,
                    }
                }
                Some("string") => {
                    allowed.extend(["format", "minLength", "maxLength"]);
                    let format = text(v, "format")?;
                    if format
                        .as_deref()
                        .is_some_and(|f| !matches!(f, "email" | "uri" | "date" | "date-time"))
                    {
                        return Err("Unsupported string format".into());
                    }
                    let min_length = count(v, "minLength")?;
                    let max_length = count(v, "maxLength")?;
                    limits(min_length, max_length)?;
                    ElicitationKind::String {
                        format,
                        min_length,
                        max_length,
                    }
                }
                Some("number" | "integer") => {
                    allowed.extend(["minimum", "maximum"]);
                    let minimum = bound(v, "minimum")?;
                    let maximum = bound(v, "maximum")?;
                    limits(minimum, maximum)?;
                    ElicitationKind::Number {
                        integer: v["type"] == "integer",
                        minimum,
                        maximum,
                    }
                }
                Some("boolean") => ElicitationKind::Boolean,
                Some("array") => {
                    allowed.extend(["items", "minItems", "maxItems"]);
                    let items = v.get("items").ok_or("Missing multiselect items")?;
                    keys(
                        items.as_object().ok_or("items must be an object")?,
                        &["type", "enum", "anyOf"],
                    )?;
                    if items.get("enum").is_some()
                        && items.get("type").and_then(Value::as_str) != Some("string")
                    {
                        return Err("Multiselect items must be strings".into());
                    }
                    if items.get("enum").is_some() && items.get("anyOf").is_some() {
                        return Err("Ambiguous multiselect schema".into());
                    }
                    let min_items = count(v, "minItems")?;
                    let max_items = count(v, "maxItems")?;
                    limits(min_items, max_items)?;
                    ElicitationKind::MultiSelect {
                        options: options(items, "anyOf")?,
                        min_items,
                        max_items,
                    }
                }
                _ => {
                    return Err(format!(
                        "Unsupported field {name}; use primitive fields or an enum multiselect"
                    ));
                }
            };
            keys(object, &allowed)?;
            let field = ElicitationField {
                name: name.clone(),
                title: text(v, "title")?.unwrap_or_else(|| name.clone()),
                description: text(v, "description")?,
                required: required.contains(&name.as_str()),
                kind,
                default: v.get("default").filter(|v| !v.is_null()).cloned(),
            };
            if let Some(default) = &field.default {
                field
                    .check(default)
                    .map_err(|e| format!("Invalid default for {name}: {e}"))?;
            }
            fields.push(field);
        }
        Ok(Self { fields })
    }
    pub fn defaults(&self) -> Map<String, Value> {
        self.fields
            .iter()
            .filter_map(|f| f.default.clone().map(|v| (f.name.clone(), v)))
            .collect()
    }
    pub fn validate(
        &self,
        content: &Map<String, Value>,
    ) -> Result<Value, Vec<ElicitationValidationError>> {
        let mut errors = Vec::new();
        let mut output = Map::new();
        for name in content
            .keys()
            .filter(|name| !self.fields.iter().any(|f| &f.name == *name))
        {
            errors.push(ElicitationValidationError {
                field: name.clone(),
                message: "Unknown field".into(),
            });
        }
        for field in &self.fields {
            let value = content
                .get(&field.name)
                .filter(|v| v.as_str() != Some("") || field.required);
            match value {
                None if field.required => errors.push(ElicitationValidationError {
                    field: field.name.clone(),
                    message: "Required".into(),
                }),
                None => {}
                Some(v) => match field.check(v) {
                    Ok(()) => {
                        output.insert(field.name.clone(), v.clone());
                    }
                    Err(message) => errors.push(ElicitationValidationError {
                        field: field.name.clone(),
                        message,
                    }),
                },
            }
        }
        if errors.is_empty() {
            Ok(Value::Object(output))
        } else {
            Err(errors)
        }
    }
}
impl ElicitationField {
    fn check(&self, v: &Value) -> Result<(), String> {
        match &self.kind {
            ElicitationKind::String {
                format,
                min_length,
                max_length,
            } => {
                let s = v.as_str().ok_or("Enter text")?;
                let n = s.chars().count();
                if min_length.is_some_and(|m| n < m) || max_length.is_some_and(|m| n > m) {
                    return Err("Text length is outside the allowed range".into());
                }
                if format.as_deref().is_some_and(|f| !valid_format(s, f)) {
                    return Err(format!(
                        "Enter a valid {}",
                        format.as_deref().unwrap_or("value")
                    ));
                }
            }
            ElicitationKind::Number {
                integer,
                minimum,
                maximum,
            } => {
                let n = v.as_f64().ok_or("Enter a number")?;
                if *integer && !(v.is_i64() || v.is_u64()) {
                    return Err("Enter an integer".into());
                }
                if minimum.is_some_and(|m| n < m) || maximum.is_some_and(|m| n > m) {
                    return Err("Number is outside the allowed range".into());
                }
            }
            ElicitationKind::Boolean => {
                if !v.is_boolean() {
                    return Err("Choose yes or no".into());
                }
            }
            ElicitationKind::Select { options } => {
                if !options.iter().any(|o| v.as_str() == Some(&o.value)) {
                    return Err("Choose an available option".into());
                }
            }
            ElicitationKind::MultiSelect {
                options,
                min_items,
                max_items,
            } => {
                let a = v.as_array().ok_or("Choose available options")?;
                if min_items.is_some_and(|m| a.len() < m) || max_items.is_some_and(|m| a.len() > m)
                {
                    return Err("Selection count is outside the allowed range".into());
                }
                for (i, v) in a.iter().enumerate() {
                    if !options.iter().any(|o| v.as_str() == Some(&o.value)) || a[..i].contains(v) {
                        return Err("Choose unique available options".into());
                    }
                }
            }
        }
        Ok(())
    }
}
fn valid_uri(s: &str) -> bool {
    let Some((scheme, rest)) = s.split_once(':') else {
        return false;
    };
    !rest.is_empty()
        && !s.chars().any(|c| c.is_whitespace() || c.is_control())
        && scheme
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}
pub fn validate_elicitation_url(s: &str) -> Result<(), String> {
    if s.chars().any(|c| c.is_whitespace() || c.is_control()) || s.contains('\\') {
        return Err("Enter a valid HTTP or HTTPS URL".into());
    }
    let parsed = ureq::get(s)
        .request_url()
        .map_err(|_| "Enter a valid HTTP or HTTPS URL")?;
    let url = parsed.as_url();
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err("Enter a valid HTTP or HTTPS URL without embedded credentials".into());
    }
    Ok(())
}

fn date(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 10
        || b[4] != b'-'
        || b[7] != b'-'
        || !b
            .iter()
            .enumerate()
            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
    {
        return false;
    }
    let y = s[..4].parse::<u32>().unwrap_or(0);
    let m = s[5..7].parse::<usize>().unwrap_or(0);
    let d = s[8..].parse::<u32>().unwrap_or(0);
    let days = [
        31,
        if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) {
            29
        } else {
            28
        },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    (1..=12).contains(&m) && d > 0 && d <= days[m - 1]
}
fn datetime(s: &str) -> bool {
    if !s.is_ascii() || s.len() < 20 || !date(&s[..10]) || !matches!(&s[10..11], "T" | "t") {
        return false;
    }
    let time = &s[11..];
    let (clock, zone) = if let Some(t) = time.strip_suffix(['Z', 'z']) {
        (t, None)
    } else if let Some(i) = time.find(['+', '-']) {
        (&time[..i], Some(&time[i + 1..]))
    } else {
        return false;
    };
    let parts = clock.split(':').collect::<Vec<_>>();
    if parts.len() != 3 {
        return false;
    }
    let num = |s: &str, max: u32| {
        s.len() == 2
            && s.bytes().all(|b| b.is_ascii_digit())
            && s.parse::<u32>().is_ok_and(|n| n <= max)
    };
    let (second, fraction) = parts[2]
        .split_once('.')
        .map_or((parts[2], None), |(s, f)| (s, Some(f)));
    num(parts[0], 23)
        && num(parts[1], 59)
        && num(second, 59)
        && fraction.is_none_or(|f| !f.is_empty() && f.bytes().all(|b| b.is_ascii_digit()))
        && zone.is_none_or(|z| {
            z.split_once(':')
                .is_some_and(|(h, m)| num(h, 23) && num(m, 59))
        })
}
fn valid_format(s: &str, f: &str) -> bool {
    match f {
        "date" => date(s),
        "date-time" => datetime(s),
        "uri" => valid_uri(s),
        "email" => s.split_once('@').is_some_and(|(a, b)| {
            !a.is_empty()
                && !b.is_empty()
                && !b.contains('@')
                && !s.chars().any(|c| c.is_whitespace() || c.is_control())
        }),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn typed_form_and_defaults() {
        let f=ElicitationForm::parse(&json!({"type":"object","properties":{"email":{"type":"string","format":"email"},"age":{"type":"integer","minimum":18},"consent":{"type":"boolean","default":false},"choice":{"type":"string","oneOf":[{"const":"a","title":"Alpha"}]},"tags":{"type":"array","items":{"anyOf":[{"const":"x","title":"X"}]},"minItems":1},"note":{"type":"string"}},"required":["email","age","consent","choice","tags"]})).unwrap();
        let input = json!({"email":"x@example.com","age":21,"consent":false,"choice":"a","tags":["x"],"note":""});
        let value = f.validate(input.as_object().unwrap()).unwrap();
        assert!(value["age"].is_i64());
        assert_eq!(value["consent"], false);
        assert!(value.get("note").is_none());
        assert_eq!(f.defaults()["consent"], false);
        let bad =
            json!({"email":"invalid","age":17,"consent":"false","choice":"b","tags":["x","x"]});
        assert_eq!(f.validate(bad.as_object().unwrap()).unwrap_err().len(), 5);
        assert_eq!(f.validate(&Map::new()).unwrap_err().len(), 5);
    }
    #[test]
    fn dates_and_bounds() {
        assert!(date("2024-02-29"));
        assert!(!date("2025-02-29"));
        assert!(!date("2025-13-01"));
        assert!(datetime("2025-01-01T23:59:59.123+05:30"));
        assert!(!datetime("2025-01-01T25:00:00Z"));
        assert!(!datetime("2025-01-01T12:00:00"));
        assert!(!datetime("é"));
    }
    #[test]
    fn unsupported_is_explicit() {
        for field in [
            json!({"type":"object","properties":{}}),
            json!({"type":"string","pattern":".*"}),
            json!({"type":"string","format":"password"}),
            json!({"type":"array","items":{"type":"string"}}),
            json!({"type":"number","minimum":5,"maximum":2}),
            json!({"type":"string","enum":["x"],"default":"y"}),
        ] {
            assert!(
                ElicitationForm::parse(&json!({"type":"object","properties":{"field":field}}))
                    .is_err()
            );
        }
    }
    #[test]
    fn legacy_enums_and_limits() {
        let f=ElicitationForm::parse(&json!({"type":"object","properties":{"s":{"type":"string","enum":["a"],"enumNames":["A"]},"m":{"type":"array","items":{"type":"string","enum":["a","b"]},"maxItems":1},"t":{"type":"string","minLength":2,"maxLength":3},"n":{"type":"number","maximum":2}}})).unwrap();
        assert!(f
            .validate(
                json!({"s":"a","m":["a"],"t":"éé","n":1.5})
                    .as_object()
                    .unwrap()
            )
            .is_ok());
        assert_eq!(
            f.validate(
                json!({"s":"b","m":["a","b"],"t":"a","n":3})
                    .as_object()
                    .unwrap()
            )
            .unwrap_err()
            .len(),
            4
        );
    }
    #[test]
    fn url_and_aliases() {
        for mode in ["form", "openaiForm", "openai/form"] {
            assert!(ElicitationRequest::parse_codex(json!(1),&json!({"mode":mode,"message":"Answer","requestedSchema":{"type":"object","properties":{}}})).is_ok());
        }
        assert!(validate_elicitation_url("https://example.com/path").is_ok());
        for url in [
            "javascript:alert(1)",
            "file:///tmp/a",
            "https://",
            "https://user:pass@example.com",
            "https://example.com/ bad",
        ] {
            assert!(validate_elicitation_url(url).is_err());
        }
    }
    #[test]
    fn typed_null_unknown_and_schema_defaults_rejected() {
        let form = ElicitationForm::parse(&json!({"type":"object","properties":{"optional":{"type":"boolean"},"required":{"type":"integer"}},"required":["required"]})).unwrap();
        assert!(form
            .validate(json!({"required":1,"optional":null}).as_object().unwrap())
            .is_err());
        assert!(form
            .validate(json!({"required":1.5}).as_object().unwrap())
            .is_err());
        assert!(form
            .validate(json!({"required":1,"unknown":true}).as_object().unwrap())
            .is_err());
        for field in [
            json!({"type":"string","format":"date","default":"2025-02-29"}),
            json!({"type":"boolean","default":"false"}),
            json!({"type":"integer","minimum":2,"default":1}),
        ] {
            assert!(
                ElicitationForm::parse(&json!({"type":"object","properties":{"x":field}})).is_err()
            );
        }
        assert!(ElicitationForm::parse(
            &json!({"type":"object","properties":{"api_key":{"type":"string"}}})
        )
        .unwrap_err()
        .contains("URL"));
    }
}
