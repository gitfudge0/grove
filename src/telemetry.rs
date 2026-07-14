//! Anonymous product telemetry (PostHog). No-ops until an API key is set.

// ponytail: paste PostHog project API key; telemetry no-ops while empty
const POSTHOG_API_KEY: &str = "phc_wECBzCsyqCpSAqgMSdFfsbR3yneyJEUPjtESi5ArsSQ7";
const POSTHOG_ENDPOINT: &str = "https://us.i.posthog.com/i/v0/e/";

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

static ENABLED: AtomicBool = AtomicBool::new(true);
static ID: OnceLock<String> = OnceLock::new();

/// Whether telemetry should currently be sent: requires a compiled-in API
/// key, respects the `GROVE_TELEMETRY=off` escape hatch, and the runtime
/// opt-out toggle in settings.
pub fn enabled() -> bool {
    if POSTHOG_API_KEY.is_empty() {
        return false;
    }
    if let Ok(v) = std::env::var("GROVE_TELEMETRY") {
        let v = v.to_lowercase();
        if v == "off" || v == "0" || v == "false" {
            return false;
        }
    }
    ENABLED.load(Ordering::Relaxed)
}

pub fn set_enabled(v: bool) {
    ENABLED.store(v, Ordering::Relaxed);
}

/// A random per-install id, persisted to `~/.config/grove/telemetry_id`
/// (or generated in memory only, if that can't be read/written).
pub fn distinct_id() -> String {
    ID.get_or_init(|| {
        if let Some(path) = telemetry_id_path() {
            if let Ok(existing) = std::fs::read_to_string(&path) {
                let trimmed = existing.trim();
                if !trimmed.is_empty() {
                    return trimmed.to_string();
                }
            }
            let id = generate_id();
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&path, &id);
            return id;
        }
        generate_id()
    })
    .clone()
}

fn telemetry_id_path() -> Option<std::path::PathBuf> {
    Some(dirs::config_dir()?.join("grove").join("telemetry_id"))
}

fn generate_id() -> String {
    let dur = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let nanos = dur.as_secs() as u128 * 1_000_000_000 + dur.subsec_nanos() as u128;
    let pid = std::process::id();
    format!("{:x}{:x}", nanos, pid)
}

/// Fire-and-forget: send `event` with `props` on a detached thread.
pub fn track(event: &'static str, props: Vec<(&'static str, serde_json::Value)>) {
    if !enabled() {
        return;
    }
    std::thread::spawn(move || send(event, props));
}

/// Same as `track`, but sends synchronously on the calling thread. Used from
/// the panic hook, where a spawned thread might not get to run before exit.
pub fn track_blocking(event: &'static str, props: Vec<(&'static str, serde_json::Value)>) {
    if !enabled() {
        return;
    }
    send(event, props);
}

fn send(event: &str, props: Vec<(&'static str, serde_json::Value)>) {
    let mut properties = serde_json::Map::new();
    properties.insert("$process_person_profile".to_string(), false.into());
    properties.insert("app_version".to_string(), env!("CARGO_PKG_VERSION").into());
    properties.insert("os".to_string(), std::env::consts::OS.into());
    for (k, v) in props {
        properties.insert(k.to_string(), v);
    }
    let body = serde_json::json!({
        "api_key": POSTHOG_API_KEY,
        "event": event,
        "distinct_id": distinct_id(),
        "properties": properties,
    });
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(3))
        .timeout_read(Duration::from_secs(5))
        .build();
    let _ = agent
        .post(POSTHOG_ENDPOINT)
        .set("User-Agent", "grove")
        .set("Content-Type", "application/json")
        .send_string(&body.to_string());
}

// ponytail: hourly ping, PostHog dedupes into DAU; no need for day-boundary logic
pub fn start_heartbeat() {
    std::thread::spawn(|| loop {
        std::thread::sleep(std::time::Duration::from_secs(3600));
        track("heartbeat", vec![]);
    });
}
