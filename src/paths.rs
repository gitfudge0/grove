//! Pure path keys shared by session and project state.
#[must_use]
pub fn path_basename(p: &str) -> String {
    std::path::Path::new(p)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(p)
        .to_string()
}

#[must_use]
pub fn normalize_wt_path(path: &str) -> &str {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        path
    } else {
        trimmed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basename_preserves_roots_and_handles_trailing_slashes() {
        assert_eq!(path_basename("/a/b/c"), "c");
        assert_eq!(path_basename("/a/b/c/"), "c");
        assert_eq!(path_basename("/"), "/");
        assert_eq!(path_basename(""), "");
        assert_eq!(path_basename("plain"), "plain");
        assert_eq!(path_basename("/a/b/../c"), "c");
    }

    #[test]
    fn worktree_key_strips_only_trailing_slashes() {
        assert_eq!(normalize_wt_path("/a/b/"), "/a/b");
        assert_eq!(normalize_wt_path("/"), "/");
        assert_eq!(normalize_wt_path(""), "");
        assert_eq!(normalize_wt_path("/a/../b"), "/a/../b");
    }
}
