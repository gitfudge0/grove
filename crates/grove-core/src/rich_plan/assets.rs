//! Plan file paths, bounded asset reads and decoded image limits.

use super::{AssetSnapshot, FileSizeLimit};
use std::{
    io::Read,
    path::{Component, Path, PathBuf},
};

fn decode_plan_destination(source: &str) -> Result<String, String> {
    let mut bytes = Vec::with_capacity(source.len());
    let mut input = source.bytes();
    while let Some(byte) = input.next() {
        if byte == b'%' {
            let mut hex = || input.next().and_then(|b| (b as char).to_digit(16));
            let high = hex().ok_or("Malformed percent escape in plan link")?;
            let low = hex().ok_or("Malformed percent escape in plan link")?;
            bytes.push((high * 16 + low) as u8);
        } else {
            bytes.push(byte);
        }
    }
    String::from_utf8(bytes).map_err(|_| "Plan link must encode valid UTF-8".into())
}

/// Resolve a local Markdown destination without changing its stored identity.
pub fn plan_link_path(base: &Path, source: &str, roots: &[PathBuf]) -> Result<PathBuf, String> {
    let destination = decode_plan_destination(source)?;
    // URL schemes are not filesystem paths. Keep native Windows drive paths.
    if let Some((scheme, _)) = destination.split_once(':') {
        let drive =
            cfg!(windows) && scheme.len() == 1 && scheme.as_bytes()[0].is_ascii_alphabetic();
        if !drive
            && !scheme.is_empty()
            && scheme.bytes().enumerate().all(|(i, b)| {
                b.is_ascii_alphabetic() || (i > 0 && (b.is_ascii_digit() || b"+-.".contains(&b)))
            })
        {
            return Err("Plan links must be local paths, not URL schemes".into());
        }
    }
    if destination.starts_with("//") {
        return Err("Plan links must not name a network host".into());
    }
    safe_path(base, &destination, roots)
}

pub fn safe_path(base: &Path, source: &str, roots: &[PathBuf]) -> Result<PathBuf, String> {
    if source.contains("://")
        || source.contains('\0')
        || Path::new(source)
            .components()
            .any(|c| matches!(c, Component::ParentDir))
    {
        return Err("Plan paths must be local without traversal".into());
    }
    let path = base
        .join(source)
        .canonicalize()
        .map_err(|e| e.to_string())?;
    if !roots
        .iter()
        .filter_map(|r| r.canonicalize().ok())
        .any(|r| path.starts_with(r))
    {
        return Err("Plan path escapes registered roots".into());
    }
    if !path.is_file() {
        return Err("Plan asset must be a regular file".into());
    }
    Ok(path)
}
#[derive(Debug)]
pub(super) struct BoundedReadError {
    pub(super) message: String,
    pub(super) size_limit: Option<FileSizeLimit>,
}
impl From<&str> for BoundedReadError {
    fn from(message: &str) -> Self {
        message.to_owned().into()
    }
}
impl From<String> for BoundedReadError {
    fn from(message: String) -> Self {
        Self {
            message,
            size_limit: None,
        }
    }
}
impl From<BoundedReadError> for String {
    fn from(error: BoundedReadError) -> Self {
        error.message
    }
}
pub(super) fn read_bounded(path: &Path, limit: usize) -> Result<Vec<u8>, BoundedReadError> {
    let f = fs_err::File::open(path).map_err(|e| e.to_string())?;
    if !f.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("Not a regular file".into());
    }
    let mut bytes = Vec::new();
    f.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > limit {
        return Err(BoundedReadError {
            message: "Plan file exceeds size limit".into(),
            size_limit: Some(FileSizeLimit {
                path: path.to_owned(),
                bytes: fs_err::metadata(path).map_or(bytes.len() as u64, |metadata| metadata.len()),
                limit: limit as u64,
            }),
        });
    }
    Ok(bytes)
}
pub(super) fn asset(
    source: &str,
    html: bool,
    assets: &mut Vec<AssetSnapshot>,
    base: &Path,
    roots: &[PathBuf],
) -> Result<(), BoundedReadError> {
    if let Some(old) = assets.iter().find(|a| a.path == source) {
        if html != (old.mime == "text/html") {
            return Err("Asset used as both image and HTML".into());
        }
        return Ok(());
    }
    if assets.len() >= 100 {
        return Err("Plan asset count exceeded".into());
    }
    let path = safe_path(base, source, roots)?;
    let remaining =
        (20usize * 1024 * 1024).saturating_sub(assets.iter().map(|a| a.bytes.len()).sum::<usize>());
    let bytes = read_bounded(
        &path,
        remaining.min(if html { 1024 * 1024 } else { 8 * 1024 * 1024 }),
    )?;
    let mime = if html {
        if !source.ends_with(".html") || std::str::from_utf8(&bytes).is_err() {
            return Err("Expected UTF-8 HTML asset".into());
        }
        "text/html"
    } else {
        raster_mime(&bytes)?
    };
    assets.push(AssetSnapshot {
        path: source.into(),
        mime: mime.into(),
        bytes: std::sync::Arc::new(bytes),
    });
    decoded_budget(assets)?;
    Ok(())
}
fn raster_mime(b: &[u8]) -> Result<&'static str, String> {
    let format = image::guess_format(b).map_err(|_| "Invalid raster image")?;
    let mime = match format {
        image::ImageFormat::Png => "image/png",
        image::ImageFormat::Jpeg => "image/jpeg",
        image::ImageFormat::WebP => "image/webp",
        _ => return Err("Supported images are PNG, JPEG and WebP".into()),
    };
    let mut reader = image::ImageReader::with_format(std::io::Cursor::new(b), format);
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(20 * 1024 * 1024);
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    reader.limits(limits);
    let decoded = reader
        .decode()
        .map_err(|e| format!("Invalid or oversized raster image: {e}"))?;
    if u64::from(decoded.width()) * u64::from(decoded.height()) * 4 > 20 * 1024 * 1024 {
        return Err("Decoded image exceeds 20 MiB".into());
    }
    Ok(mime)
}
pub(super) fn decoded_budget(assets: &[AssetSnapshot]) -> Result<(), String> {
    let mut total = 0u64;
    for asset in assets.iter().filter(|a| a.mime.starts_with("image/")) {
        let reader = image::ImageReader::new(std::io::Cursor::new(asset.bytes.as_slice()))
            .with_guessed_format()
            .map_err(|e| e.to_string())?;
        let (w, h) = reader.into_dimensions().map_err(|e| e.to_string())?;
        total = total.saturating_add(u64::from(w) * u64::from(h) * 4);
        if total > 20 * 1024 * 1024 {
            return Err("Decoded plan image budget exceeds 20 MiB".into());
        }
    }
    Ok(())
}
