//! Bounded, raster-only image preview transport shared by desktop and HTTP.
//! This is not an asset protocol: no directory scopes or executable formats
//! are exposed. Bytes are never decoded/re-encoded on the server.

use std::fs::{self, File};
use std::io::Read;
use std::path::{Component, Path};

pub const MAX_IMAGE_SIZE: u64 = 20 * 1024 * 1024;

#[derive(Debug, serde::Serialize)]
pub struct ImagePreviewError {
    pub error: String,
    pub code: &'static str,
}

impl ImagePreviewError {
    fn new(code: &'static str, error: impl Into<String>) -> Self {
        Self {
            error: error.into(),
            code,
        }
    }
}

#[derive(Debug)]
pub struct ImagePreview {
    pub bytes: Vec<u8>,
    pub mime: &'static str,
}

pub(super) fn image_mime(path: &Path) -> Result<&'static str, ImagePreviewError> {
    let extension = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match extension.as_str() {
        "png" | "apng" => Ok("image/png"),
        "jpg" | "jpeg" | "jfif" => Ok("image/jpeg"),
        "gif" => Ok("image/gif"),
        "webp" => Ok("image/webp"),
        "bmp" => Ok("image/bmp"),
        "ico" => Ok("image/x-icon"),
        "avif" => Ok("image/avif"),
        _ => Err(ImagePreviewError::new(
            "UNSUPPORTED_IMAGE_FORMAT",
            format!("Unsupported image preview format: {extension}"),
        )),
    }
}

// Confirm that the extension actually names the raster format being served.
// This deliberately does not claim to fully validate/decode a raster: malformed
// images are rejected by the browser's image decoder, never treated as HTML/SVG.
fn matches_signature(mime: &str, bytes: &[u8]) -> bool {
    match mime {
        "image/png" => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
        "image/jpeg" => bytes.starts_with(b"\xff\xd8\xff"),
        "image/gif" => bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a"),
        "image/webp" => bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP"),
        "image/bmp" => bytes.starts_with(b"BM"),
        "image/x-icon" => bytes.starts_with(b"\x00\x00\x01\x00"),
        "image/avif" => {
            // Require a complete, bounded ftyp box with an AVIF primary or
            // compatible brand. HEIC/HEIF without an AVIF brand is not AVIF.
            let Some(size) = bytes
                .get(..4)
                .and_then(|b| <[u8; 4]>::try_from(b).ok())
                .map(u32::from_be_bytes)
            else {
                return false;
            };
            let size = size as usize;
            size >= 16
                && size <= bytes.len()
                && size <= 4096
                && size % 4 == 0
                && bytes.get(4..8) == Some(b"ftyp")
                && (matches!(bytes.get(8..12), Some(b"avif" | b"avis"))
                    || bytes[16..size]
                        .chunks_exact(4)
                        .any(|brand| brand == b"avif" || brand == b"avis"))
        }
        _ => false,
    }
}

/// Only existing regular files, at most 20 MiB, with an allowlisted extension
/// AND matching raster signature. Canonicalization rejects traversal and
/// resolves symlinks; HTTP callers must additionally authorize the canonical
/// path against their ingress-specific project boundary before calling this.
pub fn load_image_preview(path: &Path) -> Result<ImagePreview, ImagePreviewError> {
    if path.components().any(|c| c == Component::ParentDir) {
        return Err(ImagePreviewError::new(
            "PATH_TRAVERSAL",
            "Image path contains '..'",
        ));
    }
    // Validate the requested extension too, so unsupported aliases cannot
    // silently become supported by pointing a .svg/.heic symlink at a PNG.
    let requested_mime = image_mime(path)?;
    let path = path
        .canonicalize()
        .map_err(|e| ImagePreviewError::new("READ_ERROR", e.to_string()))?;
    let mime = image_mime(&path)?;
    if mime != requested_mime {
        return Err(ImagePreviewError::new(
            "INVALID_IMAGE",
            "Image symlink extension does not match its target",
        ));
    }
    let metadata =
        fs::metadata(&path).map_err(|e| ImagePreviewError::new("READ_ERROR", e.to_string()))?;
    if !metadata.is_file() {
        return Err(ImagePreviewError::new(
            "READ_ERROR",
            "Image preview requires a regular file",
        ));
    }
    if metadata.len() > MAX_IMAGE_SIZE {
        return Err(ImagePreviewError::new(
            "FILE_TOO_LARGE",
            "Image exceeds the 20 MiB preview limit",
        ));
    }
    let file = File::open(path).map_err(|e| ImagePreviewError::new("READ_ERROR", e.to_string()))?;
    if !file
        .metadata()
        .map_err(|e| ImagePreviewError::new("READ_ERROR", e.to_string()))?
        .is_file()
    {
        return Err(ImagePreviewError::new(
            "READ_ERROR",
            "Image preview requires a regular file",
        ));
    }
    // take(limit + 1) also enforces the bound if the file grows after stat.
    let mut bytes = Vec::new();
    file.take(MAX_IMAGE_SIZE + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| ImagePreviewError::new("READ_ERROR", e.to_string()))?;
    if bytes.len() as u64 > MAX_IMAGE_SIZE {
        return Err(ImagePreviewError::new(
            "FILE_TOO_LARGE",
            "Image exceeds the 20 MiB preview limit",
        ));
    }
    if !matches_signature(mime, &bytes) {
        return Err(ImagePreviewError::new(
            "INVALID_IMAGE",
            "File content does not match its raster image extension",
        ));
    }
    Ok(ImagePreview { bytes, mime })
}

/// Raw binary IPC, not JSON arrays/base64 or an unrestricted asset URL.
#[tauri::command]
pub async fn read_image_preview(path: String) -> Result<tauri::ipc::Response, ImagePreviewError> {
    let image = tauri::async_runtime::spawn_blocking(move || load_image_preview(Path::new(&path)))
        .await
        .map_err(|e| ImagePreviewError::new("READ_ERROR", format!("Image task failed: {e}")))??;
    Ok(tauri::ipc::Response::new(image.bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

    struct Fixture(std::path::PathBuf);

    impl Fixture {
        fn new() -> Self {
            let unique = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let root = std::env::temp_dir().join(format!(
                "se-image-preview-{}-{unique}-{}",
                std::process::id(),
                NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&root).unwrap();
            Self(root)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn reader_enforces_size_bound_and_traversal() {
        let fixture = Fixture::new();
        let path = fixture.0.join("image.png");
        fs::write(&path, b"\x89PNG\r\n\x1a\n").unwrap();
        let file = fs::OpenOptions::new().write(true).open(&path).unwrap();
        file.set_len(MAX_IMAGE_SIZE).unwrap();
        let image = load_image_preview(&path).unwrap();
        assert_eq!(image.bytes.len() as u64, MAX_IMAGE_SIZE);
        assert_eq!(image.mime, "image/png");
        file.set_len(MAX_IMAGE_SIZE + 1).unwrap();
        let error = load_image_preview(&path).unwrap_err();
        assert_eq!(error.code, "FILE_TOO_LARGE", "{error:?}");
        assert_eq!(
            load_image_preview(&fixture.0.join("..").join("image.png"))
                .unwrap_err()
                .code,
            "PATH_TRAVERSAL"
        );
    }

    #[tokio::test]
    async fn desktop_binary_command_uses_same_reader() {
        let fixture = Fixture::new();
        let path = fixture.0.join("image.png");
        fs::write(&path, b"\x89PNG\r\n\x1a\n").unwrap();
        assert!(read_image_preview(path.to_string_lossy().into_owned())
            .await
            .is_ok());
        let path = fixture.0.join("active.png");
        fs::write(&path, b"<svg onload='alert(1)'/>").unwrap();
        assert_eq!(
            read_image_preview(path.to_string_lossy().into_owned())
                .await
                .err()
                .unwrap()
                .code,
            "INVALID_IMAGE"
        );
    }

    #[cfg(unix)]
    #[test]
    fn reader_checks_both_symlink_alias_and_target_formats() {
        use std::os::unix::fs::symlink;
        let fixture = Fixture::new();
        let target = fixture.0.join("image.png");
        fs::write(&target, b"\x89PNG\r\n\x1a\n").unwrap();
        for extension in ["svg", "heic", "jpg"] {
            let alias = fixture.0.join(format!("alias.{extension}"));
            symlink(&target, &alias).unwrap();
            assert!(load_image_preview(&alias).is_err());
        }
        let alias = fixture.0.join("safe.png");
        symlink(&target, &alias).unwrap();
        assert_eq!(load_image_preview(&alias).unwrap().mime, "image/png");
    }

    #[test]
    fn allowlist_is_explicit_and_case_insensitive() {
        for extension in [
            "png", "APNG", "JPG", "jpeg", "JFIF", "gif", "webp", "bmp", "ico", "avif",
        ] {
            assert!(image_mime(Path::new(&format!("image.{extension}"))).is_ok());
        }
        for extension in ["tif", "tiff", "heic", "heif", "svg", "html", "xml", ""] {
            assert_eq!(
                image_mime(Path::new(&format!("image.{extension}")))
                    .unwrap_err()
                    .code,
                "UNSUPPORTED_IMAGE_FORMAT"
            );
        }
    }

    #[test]
    fn apng_uses_png_signature_validation() {
        let fixture = Fixture::new();
        let path = fixture.0.join("image.APNG");
        // APNG uses the PNG signature; animation chunks stay byte-identical.
        let apng = b"\x89PNG\r\n\x1a\n\0\0\0\x08acTL\0\0\0\x02\0\0\0\0";
        fs::write(&path, apng).unwrap();
        let image = load_image_preview(&path).unwrap();
        assert_eq!(image.mime, "image/png");
        assert_eq!(image.bytes, apng);
        for bytes in [
            b"<svg onload='alert(1)'/>".as_slice(),
            b"<html><script>bad()</script>",
            b"GIF89a",
            b"\xff\xd8\xff",
        ] {
            fs::write(&path, bytes).unwrap();
            assert_eq!(load_image_preview(&path).unwrap_err().code, "INVALID_IMAGE");
        }
    }

    #[test]
    fn jfif_uses_jpeg_signature_validation() {
        let fixture = Fixture::new();
        let path = fixture.0.join("image.JFIF");
        let jpeg = b"\xff\xd8\xff\xe0\0\x10JFIF\0";
        fs::write(&path, jpeg).unwrap();
        let image = load_image_preview(&path).unwrap();
        assert_eq!(image.mime, "image/jpeg");
        assert_eq!(image.bytes, jpeg);
        for bytes in [
            b"<svg onload='alert(1)'/>".as_slice(),
            b"<html><script>bad()</script>",
            b"GIF89a",
        ] {
            fs::write(&path, bytes).unwrap();
            assert_eq!(load_image_preview(&path).unwrap_err().code, "INVALID_IMAGE");
        }
    }

    #[test]
    fn signatures_reject_active_content_and_format_mismatches() {
        let cases: &[(&str, &[u8])] = &[
            ("image/png", b"\x89PNG\r\n\x1a\n"),
            ("image/jpeg", b"\xff\xd8\xff"),
            ("image/gif", b"GIF89a"),
            ("image/webp", b"RIFF0000WEBP"),
            ("image/bmp", b"BM"),
            ("image/x-icon", b"\0\0\x01\0"),
            ("image/avif", b"\0\0\0\x10ftypavif\0\0\0\0"),
        ];
        for (mime, signature) in cases {
            assert!(matches_signature(mime, signature), "{mime}");
            assert!(!matches_signature(mime, b"<html><script>alert(1)</script>"));
            assert!(!matches_signature(mime, b"<svg onload='alert(1)'/>"));
            assert!(!matches_signature(mime, &[]));
        }
        assert!(!matches_signature("image/png", b"GIF89a"));
        assert!(!matches_signature(
            "image/avif",
            b"\0\0\0\x10ftypheic\0\0\0\0"
        ));
        assert!(!matches_signature(
            "image/avif",
            b"\0\0\x10\0ftypavif\0\0\0\0"
        ));
    }
}
