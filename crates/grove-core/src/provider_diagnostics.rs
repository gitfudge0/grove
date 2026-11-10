//! Opt-in, metadata-only protocol traces. Never persist provider payloads or stderr.
use fs_err::{File, OpenOptions};
use serde_json::{json, Value};
use std::{
    io::Write,
    path::Path,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

const MAX_BYTES: usize = 1024 * 1024;

#[derive(Clone, Default)]
pub(crate) struct Capture(Option<Arc<Mutex<Writer>>>);
struct Writer {
    file: File,
    bytes: usize,
    sequence: u64,
}

impl Capture {
    pub(crate) fn from_env(provider: &str) -> Self {
        std::env::var_os("GROVE_PROVIDER_DIAGNOSTICS_DIR")
            .filter(|path| Path::new(path).is_absolute())
            .and_then(|path| Self::open(Path::new(&path), provider).ok())
            .unwrap_or_default()
    }
    fn open(directory: &Path, provider: &str) -> std::io::Result<Self> {
        fs_err::create_dir_all(directory)?;
        let id = crate::identity::random_id().map_err(std::io::Error::other)?;
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use fs_err::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(directory.join(format!("{provider}-{id}.ndjson")))?;
        Ok(Self(Some(Arc::new(Mutex::new(Writer {
            file,
            bytes: 0,
            sequence: 0,
        })))))
    }
    pub(crate) fn record(&self, outbound: bool, value: &Value) {
        let Some(writer) = &self.0 else { return };
        let Ok(mut writer) = writer.lock() else {
            return;
        };
        if writer.bytes >= MAX_BYTES {
            return;
        }
        writer.sequence += 1;
        // Only fixed labels leave this boundary. Even unknown method/type strings,
        // request IDs, error messages, tool input and credentials are omitted.
        let label = match value["method"].as_str().or_else(|| value["type"].as_str()) {
            Some(
                label @ ("initialize"
                | "initialized"
                | "thread/start"
                | "thread/resume"
                | "thread/read"
                | "turn/start"
                | "turn/steer"
                | "turn/interrupt"
                | "turn/started"
                | "turn/completed"
                | "item/started"
                | "item/completed"
                | "item/agentMessage/delta"
                | "item/commandExecution/outputDelta"
                | "control_request"
                | "control_response"
                | "assistant"
                | "user"
                | "result"
                | "system"
                | "stream_event"
                | "rate_limit_event"),
            ) => label,
            _ if value.get("id").is_some()
                && (value.get("result").is_some() || value.get("error").is_some()) =>
            {
                "response"
            }
            _ => "other",
        };
        let label = match (label, value["subtype"].as_str(), value["state"].as_str()) {
            ("system", Some("session_state_changed"), Some("idle")) => "session_idle",
            ("system", Some("session_state_changed"), Some("running")) => "session_running",
            ("system", Some("session_state_changed"), Some("requires_action")) => {
                "session_requires_action"
            }
            _ => label,
        };
        let row = json!({"sequence":writer.sequence,"time_ms":SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis(),"direction":if outbound {"out"} else {"in"},"kind":label,"error":!value["error"].is_null() || value["is_error"] == true || value["terminal_reason"] == "api_error" || value["response"]["subtype"] == "error"});
        let Ok(mut bytes) = serde_json::to_vec(&row) else {
            return;
        };
        bytes.push(b'\n');
        if writer.bytes + bytes.len() > MAX_BYTES {
            writer.bytes = MAX_BYTES;
            return;
        }
        if writer.file.write_all(&bytes).is_err() {
            writer.bytes = MAX_BYTES;
            return;
        }
        writer.bytes += bytes.len();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn trace_omits_all_payload_and_unknown_labels() {
        let dir = tempfile::tempdir().unwrap();
        let trace = Capture::open(dir.path(), "claude").unwrap();
        trace.record(false, &json!({"type":"assistant","message":{"text":"secret-prompt","token":"secret-token"},"id":"secret-id"}));
        trace.record(
            true,
            &json!({"method":"secret-method","error":{"message":"secret-error"}}),
        );
        let path = fs_err::read_dir(dir.path())
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let text = fs_err::read_to_string(&path).unwrap();
        assert!(!text.contains("secret"));
        let rows: Vec<Value> = text
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(rows[0]["kind"], "assistant");
        assert_eq!(rows[1]["kind"], "other");
        assert_eq!(rows[1]["direction"], "out");
        assert_eq!(rows[1]["sequence"], 2);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs_err::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
    #[test]
    fn capture_distinguishes_authoritative_idle_and_api_failure() {
        let dir = tempfile::tempdir().unwrap();
        let trace = Capture::open(dir.path(), "claude").unwrap();
        trace.record(
            false,
            &json!({"type":"system", "subtype":"session_state_changed", "state":"idle"}),
        );
        trace.record(
            false,
            &json!({"type":"result", "is_error":false, "terminal_reason":"api_error"}),
        );
        let path = fs_err::read_dir(dir.path())
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let rows: Vec<Value> = fs_err::read_to_string(path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(rows[0]["kind"], "session_idle");
        assert_eq!(rows[1]["error"], true);
    }
    #[test]
    fn capture_is_bounded_and_optional() {
        Capture::default().record(false, &json!({"type":"assistant"}));
        let dir = tempfile::tempdir().unwrap();
        let trace = Capture::open(dir.path(), "codex").unwrap();
        for _ in 0..15000 {
            trace.record(false, &json!({"type":"assistant"}));
        }
        let path = fs_err::read_dir(dir.path())
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let bytes = fs_err::read(path).unwrap();
        assert!(bytes.len() <= MAX_BYTES);
        assert!(bytes.len() > MAX_BYTES - 256);
        assert_eq!(bytes.last(), Some(&b'\n'));
    }
}
