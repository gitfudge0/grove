use fs_err as fs;
use grove_core::session::Session;
use grove_core::tmux;

pub fn cycle(cur: usize, delta: i32, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    (cur as i32 + delta).rem_euclid(len as i32) as usize
}

/// Re-attach to any tmux sessions grove left running from a previous launch.
/// Sessions that fail to attach are silently dropped.
pub(crate) fn discover_sessions() -> Vec<Session> {
    tmux::list_grove_sessions()
        .into_iter()
        .filter(|d| tmux::has_session(&d.name))
        .filter_map(|d| Session::attach_existing(d).ok())
        .collect()
}

/// Last path component, or the whole string if there is no separator.
pub fn path_basename(p: &str) -> String {
    std::path::Path::new(p)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(p)
        .to_string()
}

pub fn list_dirs(buffer: &str) -> Vec<String> {
    let expanded = shellexpand_tilde(buffer);
    let (dir, prefix) = if expanded.is_empty() {
        (std::path::PathBuf::from("."), String::new())
    } else if expanded.ends_with('/') {
        (
            std::path::PathBuf::from(expanded.trim_end_matches('/')),
            String::new(),
        )
    } else {
        let pb = std::path::PathBuf::from(&expanded);
        let parent = pb
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        let name = pb
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        let parent = if parent.as_os_str().is_empty() {
            std::path::PathBuf::from(".")
        } else {
            parent
        };
        (parent, name)
    };
    let Ok(rd) = fs::read_dir(&dir) else {
        return vec![];
    };
    let mut out: Vec<String> = rd
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| !n.starts_with('.') || prefix.starts_with('.'))
        .filter(|n| n.starts_with(&prefix))
        .map(|n| format!("{}/{}", dir.display(), n))
        .collect();
    out.sort_by_key(|a| a.to_lowercase());
    out
}

pub(crate) fn shellexpand_tilde(s: &str) -> String {
    if let Some(rest) = s.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest).to_string_lossy().to_string();
        }
    }
    if s == "~" {
        if let Some(home) = dirs::home_dir() {
            return home.to_string_lossy().to_string();
        }
    }
    s.to_string()
}
