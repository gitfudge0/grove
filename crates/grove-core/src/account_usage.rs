//! Transient provider-reported account allowance, separate from conversation context.
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UsageStatus {
    Unknown,
    Allowed,
    Warning,
    Limited,
}

#[derive(Clone, Debug, PartialEq)]
pub struct UsageWindow {
    pub name: String,
    /// Missing/invalid values remain unknown, never a zero or full allowance.
    pub remaining_percent: Option<f64>,
    pub resets_at: Option<i64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AccountUsage {
    pub observed_at: u64,
    pub windows: Vec<UsageWindow>,
    /// Only a provider status establishes permission/rejection, not occupancy.
    pub status: UsageStatus,
}

pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// Absolute local reset time; expiration never fabricates a renewed allowance.
pub fn reset_label(at: i64, current: u64) -> String {
    use chrono::TimeZone as _;
    if at <= 0 {
        return "Reset time unavailable".into();
    }
    if u64::try_from(at).is_ok_and(|at| at <= current) {
        return "Reset time passed · awaiting provider update".into();
    }
    chrono::Local.timestamp_opt(at, 0).single().map_or_else(
        || "Reset time unavailable".into(),
        |time| format!("Resets {}", time.format("%a, %-d %b · %-I:%M %p")),
    )
}

fn remaining(used: Option<f64>, scale: f64) -> Option<f64> {
    used.filter(|n| n.is_finite() && (0.0..=scale).contains(n))
        .map(|n| (1.0 - n / scale) * 100.0)
}

fn reset(value: &Value) -> Option<i64> {
    value["resetsAt"]
        .as_i64()
        .or(value["resets_at"].as_i64())
        .filter(|n| *n > 0)
}

fn duration_label(window: &Value, fallback: &str) -> String {
    match window["windowDurationMins"].as_u64() {
        Some(10080) => "Weekly".into(),
        Some(60) => "1 hour".into(),
        Some(n) if n > 0 && n % 60 == 0 => format!("{} hours", n / 60),
        Some(n) if n > 0 => format!("{n} minutes"),
        _ => fallback.into(),
    }
}

fn codex_windows(snapshot: &Value, prefix: Option<&str>, windows: &mut Vec<UsageWindow>) {
    for (key, fallback) in [
        ("primary", "Primary window"),
        ("secondary", "Secondary window"),
    ] {
        let window = &snapshot[key];
        if !window.is_object() {
            continue;
        }
        let label = duration_label(window, fallback);
        windows.push(UsageWindow {
            name: prefix.map_or_else(|| label.clone(), |prefix| format!("{prefix} · {label}")),
            remaining_percent: remaining(window["usedPercent"].as_f64(), 100.0),
            resets_at: reset(window),
        });
    }
}

/// Accepts a rate-limit read response or the parameters of a rate-limit update.
/// A complete per-limit map supersedes the compatibility snapshot to avoid duplicates.
pub fn codex(value: &Value, observed_at: u64) -> Option<AccountUsage> {
    let mut windows = Vec::new();
    if let Some(limits) = value["rateLimitsByLimitId"]
        .as_object()
        .filter(|m| !m.is_empty())
    {
        for (id, snapshot) in limits {
            let name = snapshot["limitName"]
                .as_str()
                .filter(|s| !s.is_empty())
                .unwrap_or(id);
            codex_windows(snapshot, Some(name), &mut windows);
        }
    } else {
        codex_windows(value.get("rateLimits").unwrap_or(value), None, &mut windows);
    }
    let status = match value["ordinaryUsageAllowed"].as_bool() {
        Some(true) => UsageStatus::Allowed,
        Some(false) => UsageStatus::Limited,
        None => UsageStatus::Unknown,
    };
    (!windows.is_empty() || status != UsageStatus::Unknown).then_some(AccountUsage {
        observed_at,
        windows,
        status,
    })
}

fn claude_label(kind: &str) -> String {
    match kind {
        "five_hour" => "5 hours".into(),
        "seven_day" => "Weekly".into(),
        "seven_day_oauth_apps" => "Weekly · OAuth apps".into(),
        "seven_day_opus" => "Weekly · Opus".into(),
        "seven_day_sonnet" => "Weekly · Sonnet".into(),
        "overage" => "Extra usage".into(),
        _ => kind.replace('_', " "),
    }
}

/// Claude's /usage control response reports percentages on a 0–100 scale and
/// RFC 3339 reset times. The conversation rate-limit events use different units.
pub(crate) fn claude_read(value: &Value, observed_at: u64) -> Option<AccountUsage> {
    if value["rate_limits_available"].as_bool() != Some(true) {
        return None;
    }
    let limits = &value["rate_limits"];
    let make_window = |name, value: &Value| {
        let remaining_percent = remaining(value["utilization"].as_f64(), 100.0);
        let resets_at = value["resets_at"]
            .as_str()
            .and_then(|time| chrono::DateTime::parse_from_rfc3339(time).ok())
            .map(|time| time.timestamp())
            .filter(|time| *time > 0);
        (remaining_percent.is_some() || resets_at.is_some()).then_some(UsageWindow {
            name,
            remaining_percent,
            resets_at,
        })
    };
    let mut windows: Vec<_> = [
        "five_hour",
        "seven_day",
        "seven_day_oauth_apps",
        "seven_day_opus",
        "seven_day_sonnet",
    ]
    .into_iter()
    .filter_map(|kind| make_window(claude_label(kind), &limits[kind]))
    .collect();
    if let Some(models) = limits["model_scoped"].as_array() {
        for model in models {
            let Some(label) = model["display_name"].as_str() else {
                continue;
            };
            let label: String = label
                .chars()
                .filter(|ch| !ch.is_control())
                .take(80)
                .collect();
            let label = label.trim();
            if label.is_empty() {
                continue;
            }
            let name = format!("Weekly · {label}");
            if !windows.iter().any(|window| window.name == name) {
                windows.extend(make_window(name, model));
            }
        }
    }
    (!windows.is_empty()).then_some(AccountUsage {
        observed_at,
        windows,
        status: UsageStatus::Unknown,
    })
}

/// A Claude event is one authoritative observation. Do not merge old percentages
/// into a new event which omits them, or claim that one bucket refreshes another.
pub fn claude(info: &Value, observed_at: u64) -> Option<AccountUsage> {
    let status = match info["status"].as_str() {
        Some("allowed") => UsageStatus::Allowed,
        Some("allowed_warning") => UsageStatus::Warning,
        Some("rejected") => UsageStatus::Limited,
        _ => UsageStatus::Unknown,
    };
    let mut windows = Vec::new();
    let make_window = |kind: &str, value: &Value| UsageWindow {
        name: claude_label(kind),
        remaining_percent: remaining(value["utilization"].as_f64(), 1.0),
        resets_at: reset(value),
    };
    // Some CLI versions include a map of all reported windows. Older versions
    // report only the bucket associated with this status transition.
    if let Some(buckets) = info["unifiedWindows"]
        .as_object()
        .or(info["unified_windows"].as_object())
    {
        for (kind, value) in buckets {
            if value.is_object() {
                windows.push(make_window(kind, value));
            }
        }
    }
    if windows.is_empty() {
        if let Some(kind) = info["rateLimitType"]
            .as_str()
            .or(info["rate_limit_type"].as_str())
        {
            windows.push(make_window(kind, info));
        }
    }
    (!windows.is_empty() || status != UsageStatus::Unknown).then_some(AccountUsage {
        observed_at,
        windows,
        status,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn expired_reset_never_implies_a_new_allowance() {
        assert_eq!(
            reset_label(123, 124),
            "Reset time passed · awaiting provider update"
        );
        assert_eq!(reset_label(0, 124), "Reset time unavailable");
        assert!(reset_label(200, 124).starts_with("Resets "));
    }

    #[test]
    fn codex_keeps_all_windows_without_duplicating_compatibility_snapshot() {
        let usage = codex(&json!({"rateLimits":{"primary":{"usedPercent":1}},"rateLimitsByLimitId":{
            "codex":{"primary":{"usedPercent":38,"windowDurationMins":300,"resetsAt":123},"secondary":{"usedPercent":72,"windowDurationMins":10080}},
            "spark":{"limitName":"Spark","primary":{"usedPercent":0}}}}), 8).unwrap();
        assert_eq!(usage.windows.len(), 3);
        assert_eq!(usage.windows[0].name, "codex · 5 hours");
        assert_eq!(usage.windows[0].remaining_percent, Some(62.0));
        assert_eq!(usage.windows[0].resets_at, Some(123));
        assert_eq!(usage.windows[1].name, "codex · Weekly");
        assert_eq!(usage.windows[2].remaining_percent, Some(100.0));
        assert_eq!(usage.status, UsageStatus::Unknown);
    }

    #[test]
    fn occupancy_does_not_invent_a_rejection_or_missing_percentage() {
        let usage = codex(
            &json!({"primary":{"usedPercent":100},"secondary":{"usedPercent":140}}),
            8,
        )
        .unwrap();
        assert_eq!(usage.status, UsageStatus::Unknown);
        assert_eq!(usage.windows[0].remaining_percent, Some(0.0));
        assert_eq!(usage.windows[1].remaining_percent, None);
        assert!(codex(&json!({}), 8).is_none());
    }

    #[test]
    fn claude_unknown_percent_and_replacement_are_explicit() {
        let known=claude(&json!({"status":"allowed_warning","rateLimitType":"five_hour","utilization":0.92,"resetsAt":123}), 8).unwrap();
        assert!((known.windows[0].remaining_percent.unwrap() - 8.0).abs() < 0.00001);
        assert_eq!(known.status, UsageStatus::Warning);
        let newer = claude(&json!({"status":"allowed","rateLimitType":"five_hour"}), 9).unwrap();
        assert_eq!(newer.windows[0].remaining_percent, None);
        assert_eq!(newer.windows[0].resets_at, None);
        assert_eq!(newer.status, UsageStatus::Allowed);
        assert_eq!(newer.observed_at, 9);
    }

    #[test]
    fn claude_multiwindow_payload_preserves_named_limits_and_invalid_values() {
        let usage = claude(
            &json!({"status":"allowed","unifiedWindows":{
            "five_hour":{"utilization":0.2,"resets_at":123},
            "seven_day_opus":{"utilization":1.1},
            "seven_day_sonnet":{"utilization":0}}}),
            8,
        )
        .unwrap();
        assert_eq!(usage.windows.len(), 3);
        assert_eq!(usage.windows[0].remaining_percent, Some(80.0));
        assert_eq!(usage.windows[1].name, "Weekly · Opus");
        assert_eq!(usage.windows[1].remaining_percent, None);
        assert_eq!(usage.windows[2].remaining_percent, Some(100.0));
    }

    #[test]
    fn claude_read_uses_percentages_and_absolute_reset_times() {
        let usage = claude_read(
            &json!({"rate_limits_available":true,"rate_limits":{
                "five_hour":{"utilization":38,"resets_at":"2026-11-20T10:00:00Z"},
                "seven_day":{"utilization":100,"resets_at":null},
                "seven_day_oauth_apps":{"utilization":null,"resets_at":"2026-11-20T15:30:00+05:30"},
                "seven_day_opus":{"utilization":101,"resets_at":"invalid"},
                "model_scoped":[{"display_name":"Fable","utilization":25,"resets_at":null}]
            }}),
            8,
        )
        .unwrap();
        assert_eq!(usage.observed_at, 8);
        assert_eq!(usage.status, UsageStatus::Unknown);
        assert_eq!(usage.windows.len(), 4);
        assert_eq!(usage.windows[0].remaining_percent, Some(62.0));
        assert_eq!(usage.windows[0].resets_at, Some(1795168800));
        assert_eq!(usage.windows[1].remaining_percent, Some(0.0));
        assert_eq!(usage.windows[2].remaining_percent, None);
        assert_eq!(usage.windows[2].resets_at, usage.windows[0].resets_at);
        assert_eq!(usage.windows[3].name, "Weekly · Fable");
        assert_eq!(usage.windows[3].remaining_percent, Some(75.0));
    }

    #[test]
    fn claude_read_without_plan_allowance_returns_no_measurement() {
        for response in [
            json!({"rate_limits_available":false,"rate_limits":{"five_hour":{"utilization":0}}}),
            json!({"rate_limits_available":true,"rate_limits":null}),
            json!({"rate_limits_available":true,"rate_limits":{"five_hour":{"utilization":-1}}}),
        ] {
            assert!(claude_read(&response, 8).is_none());
        }
    }
}
