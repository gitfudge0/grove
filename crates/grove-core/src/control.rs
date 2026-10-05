//! Versioned local control protocol. The desktop remains the session owner.
use crate::agent::Agent;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

pub const VERSION: u32 = 1;
const MAX_FRAME: usize = 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControlRequest {
    pub version: u32,
    pub request_id: String,
    pub command: ControlCommand,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "snake_case", deny_unknown_fields)]
pub enum ControlCommand {
    ListProjects,
    ListWorktrees {
        project: String,
    },
    CreateWorktree {
        project: String,
        name: String,
        base: Option<String>,
    },
    ListSessions,
    ShowSession {
        id: String,
    },
    StartSession {
        project: String,
        worktree: String,
        agent: Agent,
        prompt: Option<String>,
        backend: Option<bool>,
        task: Option<TaskSpec>,
    },
    Logs {
        id: String,
        lines: usize,
    },
    Focus {
        id: String,
    },
    Stop {
        id: String,
    },
    ListTasks,
    ShowTask {
        id: String,
    },
    CompleteTask {
        id: String,
        result: TaskResult,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskSpec {
    pub title: String,
    pub instructions: String,
    pub parent: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskResult {
    pub summary: String,
    pub changed_files: Vec<String>,
    pub checks: Vec<String>,
    pub unresolved: Vec<String>,
    pub status: String,
}
impl TaskResult {
    pub fn validate(&self) -> Result<(), String> {
        if !matches!(self.status.as_str(), "completed" | "failed") {
            return Err("Task result status must be completed or failed".into());
        }
        if self.summary.trim().is_empty() {
            return Err("Task result summary must not be empty".into());
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ControlError {
    pub code: String,
    pub message: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ControlResponse {
    pub version: u32,
    pub ok: bool,
    pub data: Value,
    pub error: Option<ControlError>,
}
impl ControlResponse {
    pub fn success(data: Value) -> Self {
        Self {
            version: VERSION,
            ok: true,
            data,
            error: None,
        }
    }
    pub fn error(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            version: VERSION,
            ok: false,
            data: Value::Null,
            error: Some(ControlError {
                code: code.into(),
                message: message.into(),
            }),
        }
    }
}
pub fn random_id() -> Result<String, String> {
    use ring::rand::{SecureRandom, SystemRandom};
    const HEX: &[u8] = b"0123456789abcdef";
    let mut bytes = [0u8; 16];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| "Secure random generator failed".to_owned())?;
    let mut id = String::with_capacity(32);
    for byte in bytes {
        id.push(char::from(HEX[usize::from(byte >> 4)]));
        id.push(char::from(HEX[usize::from(byte & 15)]));
    }
    Ok(id)
}
pub fn socket_path() -> Result<PathBuf, String> {
    crate::storage::config_dir()
        .map(|p| p.join("control/control.sock"))
        .map_err(|e| e.to_string())
}

#[cfg(unix)]
mod transport {
    use super::{ControlRequest, ControlResponse, MAX_FRAME, VERSION};
    use fs_err::{self as fs, os::unix::fs::OpenOptionsExt, File, OpenOptions};
    use std::{
        fs::Permissions,
        io::{Read, Write},
        os::{
            fd::AsRawFd,
            unix::{
                fs::{FileTypeExt, MetadataExt, PermissionsExt},
                net::{UnixListener, UnixStream},
            },
        },
        path::{Path, PathBuf},
        time::Duration,
    };
    pub struct Listener {
        socket: UnixListener,
        path: PathBuf,
        inode: u64,
        _lock: File,
    }
    pub struct Connection {
        stream: UnixStream,
    }
    impl Listener {
        pub fn bind() -> Result<Self, String> {
            Self::bind_at(&super::socket_path()?)
        }
        fn bind_at(path: &Path) -> Result<Self, String> {
            let parent = path.parent().ok_or("Socket needs a parent directory")?;
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            let meta = fs::symlink_metadata(parent).map_err(|e| e.to_string())?;
            if !meta.is_dir() || meta.uid() != unsafe { libc::geteuid() } {
                return Err("Control directory must be owned by the current user".into());
            }
            fs::set_permissions(parent, Permissions::from_mode(0o700))
                .map_err(|e| e.to_string())?;
            let lock = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW)
                .open(parent.join("owner.lock"))
                .map_err(|e| e.to_string())?;
            // Keep ownership across stale endpoint inspection, bind and cleanup.
            if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
                return Err("Another Grove instance owns the control endpoint".into());
            }
            if path.exists() {
                if !fs::symlink_metadata(path)
                    .map_err(|e| e.to_string())?
                    .file_type()
                    .is_socket()
                {
                    return Err("Control endpoint exists and is not a socket".into());
                }
                match UnixStream::connect(path) {
                    Ok(_) => return Err("Another Grove instance is listening".into()),
                    Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => {
                        fs::remove_file(path).map_err(|e| e.to_string())?;
                    }
                    Err(e) => return Err(format!("Cannot inspect control endpoint: {e}")),
                }
            }
            let listener = UnixListener::bind(path).map_err(|e| e.to_string())?;
            fs::set_permissions(path, Permissions::from_mode(0o600)).map_err(|e| e.to_string())?;
            let inode = fs::symlink_metadata(path).map_err(|e| e.to_string())?.ino();
            Ok(Self {
                socket: listener,
                path: path.to_owned(),
                inode,
                _lock: lock,
            })
        }
        pub fn accept(&self) -> std::io::Result<Connection> {
            let (stream, _) = self.socket.accept()?;
            Connection::new(stream)
        }
        pub fn set_nonblocking(&self, value: bool) -> Result<(), String> {
            self.socket
                .set_nonblocking(value)
                .map_err(|e| e.to_string())
        }
    }
    impl Drop for Listener {
        fn drop(&mut self) {
            if fs::symlink_metadata(&self.path).is_ok_and(|m| m.ino() == self.inode) {
                let _ = fs::remove_file(&self.path);
            }
        }
    }
    impl Connection {
        fn new(stream: UnixStream) -> std::io::Result<Self> {
            stream.set_read_timeout(Some(Duration::from_secs(15)))?;
            stream.set_write_timeout(Some(Duration::from_secs(15)))?;
            Ok(Self { stream })
        }
        pub fn read_request(&mut self) -> Result<ControlRequest, String> {
            read_frame(&mut self.stream)
        }
        pub fn write_response(&mut self, response: &ControlResponse) -> Result<(), String> {
            write_frame(&mut self.stream, response)
        }
    }
    fn read_frame<T: serde::de::DeserializeOwned>(stream: &mut impl Read) -> Result<T, String> {
        let mut header = [0u8; 4];
        stream.read_exact(&mut header).map_err(|e| e.to_string())?;
        let size = u32::from_be_bytes(header) as usize;
        if size == 0 || size > MAX_FRAME {
            return Err("Control frame exceeds allowed size".into());
        }
        let mut bytes = vec![0; size];
        stream.read_exact(&mut bytes).map_err(|e| e.to_string())?;
        serde_json::from_slice(&bytes).map_err(|e| e.to_string())
    }
    fn write_frame<T: serde::Serialize>(stream: &mut impl Write, value: &T) -> Result<(), String> {
        let bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?;
        if bytes.len() > MAX_FRAME {
            return Err("Control frame exceeds allowed size".into());
        }
        stream
            .write_all(&(bytes.len() as u32).to_be_bytes())
            .and_then(|()| stream.write_all(&bytes))
            .map_err(|e| e.to_string())
    }
    pub fn send(request: &ControlRequest) -> Result<ControlResponse, String> {
        let stream = UnixStream::connect(super::socket_path()?)
            .map_err(|e| format!("Cannot connect to Grove; open the desktop app first: {e}"))?;
        let mut connection = Connection::new(stream).map_err(|e| e.to_string())?;
        write_frame(&mut connection.stream, request)?;
        let response: ControlResponse = read_frame(&mut connection.stream)
            .map_err(|e| format!("Request outcome uncertain: response unavailable: {e}"))?;
        if response.version != VERSION {
            return Err("Grove control protocol version mismatch".into());
        }
        Ok(response)
    }
    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn rejects_oversized_and_truncated_frames() {
            let mut oversized = ((MAX_FRAME + 1) as u32).to_be_bytes().as_slice().to_owned();
            assert!(read_frame::<ControlRequest>(&mut oversized.as_slice()).is_err());
            oversized = vec![0, 0, 0, 4, b'{'];
            assert!(read_frame::<ControlRequest>(&mut oversized.as_slice()).is_err());
        }
        #[test]
        fn recovers_stale_socket_but_preserves_other_files() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("private/control.sock");
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            let stale = UnixListener::bind(&path).unwrap();
            drop(stale);
            let recovered = Listener::bind_at(&path).unwrap();
            drop(recovered);
            fs::write(&path, "keep").unwrap();
            assert!(Listener::bind_at(&path).is_err());
            assert_eq!(fs::read_to_string(&path).unwrap(), "keep");
        }
        #[test]
        fn private_listener_excludes_second_owner_and_roundtrips() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("private/control.sock");
            let listener = Listener::bind_at(&path).unwrap();
            assert!(Listener::bind_at(&path).is_err());
            assert_eq!(
                fs::metadata(path.parent().unwrap()).unwrap().mode() & 0o777,
                0o700
            );
            let client = std::thread::spawn({
                let path = path.clone();
                move || {
                    let mut stream = UnixStream::connect(path).unwrap();
                    write_frame(
                        &mut stream,
                        &ControlRequest {
                            version: VERSION,
                            request_id: "test".into(),
                            command: crate::control::ControlCommand::ListProjects,
                        },
                    )
                    .unwrap();
                    read_frame::<ControlResponse>(&mut stream).unwrap()
                }
            });
            let mut connection = listener.accept().unwrap();
            assert_eq!(connection.read_request().unwrap().request_id, "test");
            connection
                .write_response(&ControlResponse::success(serde_json::json!([])))
                .unwrap();
            assert!(client.join().unwrap().ok);
            drop(listener);
            assert!(!path.exists());
            assert!(Listener::bind_at(&path).is_ok());
        }
    }
}
#[cfg(unix)]
pub use transport::{send, Connection, Listener};
#[cfg(not(unix))]
pub struct Listener;
#[cfg(not(unix))]
pub struct Connection;
#[cfg(not(unix))]
impl Listener {
    pub fn bind() -> Result<Self, String> {
        Err("Local CLI control is not yet supported on this platform".into())
    }
    pub fn accept(&self) -> std::io::Result<Connection> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "Unsupported platform",
        ))
    }
    pub fn set_nonblocking(&self, _: bool) -> Result<(), String> {
        Err("Unsupported platform".into())
    }
}
#[cfg(not(unix))]
impl Connection {
    pub fn read_request(&mut self) -> Result<ControlRequest, String> {
        Err("Unsupported platform".into())
    }
    pub fn write_response(&mut self, _: &ControlResponse) -> Result<(), String> {
        Err("Unsupported platform".into())
    }
}
#[cfg(not(unix))]
pub fn send(_: &ControlRequest) -> Result<ControlResponse, String> {
    Err("Local CLI control is not yet supported on this platform".into())
}
