//! Anonymous product telemetry (PostHog). No-ops until an API key is set.
//!
//! A behavior-verbatim port of the iced app's `src/telemetry.rs`.
//! Deliberately **not** hoisted into grove-core: iced is deleted in
//! Plan 10, and a two-month-old shared module is not worth the amendment
//! (rewrite Constraint 3, foreseen candidate 2). No gpui types live here.

// Supplied at build time via `GROVE_POSTHOG_KEY`; telemetry no-ops when unset.
const POSTHOG_API_KEY: Option<&str> = option_env!("GROVE_POSTHOG_KEY");
const POSTHOG_ENDPOINT: &str = "https://us.i.posthog.com/i/v0/e/";

use fs_err as fs;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Starts `false` so nothing is transmitted before the stored
/// `telemetry_enabled` preference has been read; `set_enabled` flips it on at
/// startup once the store is loaded.
static ENABLED: AtomicBool = AtomicBool::new(false);
static ID: OnceLock<String> = OnceLock::new();

/// Whether telemetry should currently be sent: requires a compiled-in API
/// key, respects the `GROVE_TELEMETRY=off` escape hatch, and the runtime
/// opt-out toggle in settings.
pub fn enabled() -> bool {
    if api_key().is_none() {
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

/// The compiled-in PostHog key, if one was baked in. `None` (or an empty
/// value) turns every send path into a no-op.
fn api_key() -> Option<&'static str> {
    POSTHOG_API_KEY.filter(|k| !k.is_empty())
}

pub fn set_enabled(v: bool) {
    ENABLED.store(v, Ordering::Relaxed);
}

/// A random per-install id, persisted to `~/.config/grove/telemetry_id`
/// (or generated in memory only, if that can't be read/written).
pub fn distinct_id() -> String {
    ID.get_or_init(|| {
        if let Some(path) = telemetry_id_path() {
            if let Ok(existing) = fs::read_to_string(&path) {
                let trimmed = existing.trim();
                if !trimmed.is_empty() {
                    return trimmed.to_string();
                }
            }
            let id = generate_id();
            if let Some(parent) = path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            if let Err(e) = fs::write(&path, &id) {
                tracing::debug!(error = %e, "failed to persist telemetry id");
            }
            return id;
        }
        generate_id()
    })
    .clone()
}

fn telemetry_id_path() -> Option<std::path::PathBuf> {
    // Routed through grove-core so `GROVE_CONFIG_DIR` is honored.
    grove_core::storage::config_dir()
        .ok()
        .map(|d| d.join("telemetry_id"))
}

fn generate_id() -> String {
    let dur = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let nanos = dur.as_secs() as u128 * 1_000_000_000 + dur.subsec_nanos() as u128;
    let pid = std::process::id();
    format!("{nanos:x}{pid:x}")
}

/// Strip filesystem identity out of a string before it leaves the machine
/// (used on the panic *location*, never on the panic message — the free-text
/// payload is never transmitted): the user's home directory collapses to `~`,
/// and any other
/// absolute path is replaced wholesale with `<path>`. Relative paths — which
/// is what panic locations look like (`src/app/mod.rs:12:5`) — are kept.
pub fn scrub_paths(msg: &str) -> String {
    let home = dirs::home_dir().and_then(|h| h.to_str().map(str::to_string));
    let mut out = String::with_capacity(msg.len());
    for (i, token) in msg.split(' ').enumerate() {
        if i > 0 {
            out.push(' ');
        }
        out.push_str(&scrub_token(token, home.as_deref()));
    }
    out
}

/// A token is path-looking if it starts with `/` or with the home directory
/// (possibly behind common punctuation such as a quote or an opening paren).
fn scrub_token(token: &str, home: Option<&str>) -> String {
    let lead = token.len() - token.trim_start_matches(['"', '\'', '`', '(', '[']).len();
    let (prefix, rest) = token.split_at(lead);
    if let Some(home) = home {
        if let Some(tail) = rest.strip_prefix(home) {
            if tail.is_empty() || tail.starts_with('/') {
                return format!("{prefix}~{tail}");
            }
        }
    }
    if rest.starts_with('/') {
        return format!("{prefix}<path>");
    }
    token.to_string()
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
    let Some(api_key) = api_key() else {
        return;
    };
    let mut properties = serde_json::Map::new();
    properties.insert("$process_person_profile".to_string(), false.into());
    properties.insert("app_version".to_string(), env!("CARGO_PKG_VERSION").into());
    properties.insert("os".to_string(), std::env::consts::OS.into());
    for (k, v) in props {
        properties.insert(k.to_string(), v);
    }
    let body = serde_json::json!({
        "api_key": api_key,
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
        std::thread::sleep(Duration::from_hours(1));
        track("heartbeat", vec![]);
    });
}

/// The scrubbing panic hook (`src/main.rs:11-30`). Installed from `main`
/// **before** `app::boot`, so a panic inside boot is still reported.
///
/// The panic *message* never leaves the machine — it is logged locally and
/// only the scrubbed `file:line:col` location is transmitted. `track_blocking`
/// because a thread spawned from a panicking process may never get to run.
pub fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let msg = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| (*s).to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown panic".to_string());
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_default();
        tracing::error!(panic = %msg, location = %location, "grove-gpui panicked");
        track_blocking("panic", vec![("location", scrub_paths(&location).into())]);
        previous(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::scrub_token;

    fn scrub(msg: &str) -> String {
        msg.split(' ')
            .map(|t| scrub_token(t, Some("/home/tester")))
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn home_collapses_to_tilde() {
        assert_eq!(
            scrub("open /home/tester/dev/x failed"),
            "open ~/dev/x failed"
        );
        assert_eq!(scrub("/home/tester"), "~");
    }

    #[test]
    fn other_absolute_paths_are_redacted() {
        assert_eq!(scrub("no /etc/secret/key here"), "no <path> here");
        assert_eq!(scrub("\"/home/other/x\""), "\"<path>");
    }

    #[test]
    fn panic_location_survives() {
        let msg = "called on None, src/app/mod.rs:12:5";
        assert_eq!(scrub(msg), msg);
    }

    #[test]
    fn home_prefix_is_not_matched_mid_segment() {
        assert_eq!(scrub("/home/tester2/x"), "<path>");
    }

    /// Three gates, and the first one is a compile-time constant: no key is
    /// baked into a test build, so nothing can leave the machine no matter
    /// what the runtime toggle says (carried decision 3).
    #[test]
    fn nothing_is_transmitted_without_a_compiled_in_key() {
        assert!(super::api_key().is_none());
        super::set_enabled(true);
        assert!(!super::enabled());
        super::set_enabled(false);
    }
}
