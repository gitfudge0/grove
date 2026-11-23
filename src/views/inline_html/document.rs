//! Bounded loading of local HTML documents.

use std::io::Read as _;
use std::path::Path;

pub(super) const MAX_HTML_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug)]
pub(super) struct DocumentReadError {
    pub(super) message: String,
    pub(super) size_limit: Option<grove_core::rich_plan::FileSizeLimit>,
}
impl From<String> for DocumentReadError {
    fn from(message: String) -> Self {
        Self {
            message,
            size_limit: None,
        }
    }
}
impl From<&str> for DocumentReadError {
    fn from(message: &str) -> Self {
        message.to_owned().into()
    }
}
pub(super) fn read_document(path: &Path) -> Result<String, DocumentReadError> {
    if !fs_err::metadata(path)
        .map_err(|e| format!("Couldn’t read HTML: {e}"))?
        .is_file()
    {
        return Err("HTML target must be a regular file".into());
    }
    let mut options = fs_err::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use fs_err::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = options
        .open(path)
        .map_err(|e| format!("Couldn’t read HTML: {e}"))?;
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("HTML target must be a regular file".into());
    }
    let mut bytes = Vec::new();
    file.take((MAX_HTML_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > MAX_HTML_BYTES {
        return Err(DocumentReadError {
            message: "HTML document exceeds the 2 MiB preview limit".into(),
            size_limit: Some(grove_core::rich_plan::FileSizeLimit {
                path: path.to_owned(),
                bytes: fs_err::metadata(path).map_or(bytes.len() as u64, |metadata| metadata.len()),
                limit: MAX_HTML_BYTES as u64,
            }),
        });
    }
    String::from_utf8(bytes).map_err(|_| "HTML preview requires UTF-8 text".into())
}
