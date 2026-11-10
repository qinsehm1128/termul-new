//! Host-backed staging of remote screenshot attachments.
//!
//! A paired phone can capture a region of the remote desktop but a terminal
//! (claude / pi / qin running in a PTY) only consumes text. This service writes
//! the uploaded raster into an app-owned temporary directory and returns its
//! absolute path so the client can type that path into the terminal over the
//! existing terminal WS `write` frame.
//!
//! Chat (ACP) sessions do not need this path: they already accept inline image
//! content blocks.
//!
//! Policy:
//! - PNG or JPEG only, verified from magic bytes; dimensions are read from the
//!   container header (never a full decode).
//! - At most [`MAX_ATTACHMENT_BYTES`] and [`MAX_ATTACHMENT_DIMENSION`] per side.
//! - Files older than [`MAX_ATTACHMENT_AGE`] are deleted on every stage call and
//!   at startup; at most [`MAX_ATTACHMENT_FILES`] are retained.
//! - Unix: directory mode `0700`, file mode `0600`. Windows: default ACLs.
//!
//! The service is shared by the desktop shared-live host and `se-server`; it has
//! no renderer/Tauri counterpart (desktop paste uses the host clipboard).

use std::fs::{self, OpenOptions};
use std::io::{self, Cursor, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Maximum accepted raster payload.
pub const MAX_ATTACHMENT_BYTES: usize = 8 * 1024 * 1024;
/// Maximum accepted width or height.
pub const MAX_ATTACHMENT_DIMENSION: u32 = 4096;
/// Maximum number of staged files retained in the directory.
pub const MAX_ATTACHMENT_FILES: usize = 50;
/// Staged files older than this are deleted.
pub const MAX_ATTACHMENT_AGE: Duration = Duration::from_secs(24 * 60 * 60);

const FILE_PREFIX: &str = "se-shot-";
const DIRECTORY_NAME: &str = "se-remote-attachments";

/// Supported raster containers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageKind {
    Png,
    Jpeg,
}

impl ImageKind {
    /// Parse the `Content-Type` value (parameters and case ignored).
    #[must_use]
    pub fn from_content_type(value: &str) -> Option<Self> {
        match value
            .split(';')
            .next()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "image/png" => Some(Self::Png),
            "image/jpeg" => Some(Self::Jpeg),
            _ => None,
        }
    }

    /// Detect the container from its magic bytes.
    #[must_use]
    pub fn from_magic(bytes: &[u8]) -> Option<Self> {
        if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]) {
            Some(Self::Png)
        } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
            Some(Self::Jpeg)
        } else {
            None
        }
    }

    #[must_use]
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
        }
    }

    #[must_use]
    pub const fn mime(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
        }
    }
}

/// Staging failure. `code` is the stable client-facing identifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttachmentError {
    Empty,
    TooLarge { size: usize, limit: usize },
    UnsupportedMediaType,
    ContentTypeMismatch,
    InvalidImage(&'static str),
    DimensionsTooLarge { width: u32, height: u32 },
    Io(String),
}

impl AttachmentError {
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Empty => "EMPTY_BODY",
            Self::TooLarge { .. } => "PAYLOAD_TOO_LARGE",
            Self::UnsupportedMediaType => "UNSUPPORTED_MEDIA_TYPE",
            Self::ContentTypeMismatch => "CONTENT_TYPE_MISMATCH",
            Self::InvalidImage(_) => "INVALID_IMAGE",
            Self::DimensionsTooLarge { .. } => "IMAGE_DIMENSIONS_TOO_LARGE",
            Self::Io(_) => "STAGE_FAILED",
        }
    }
}

impl std::fmt::Display for AttachmentError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => write!(formatter, "attachment body is empty"),
            Self::TooLarge { size, limit } => write!(
                formatter,
                "attachment is {size} bytes; the limit is {limit} bytes"
            ),
            Self::UnsupportedMediaType => {
                write!(formatter, "attachment must be PNG or JPEG")
            }
            Self::ContentTypeMismatch => {
                write!(formatter, "attachment bytes do not match Content-Type")
            }
            Self::InvalidImage(reason) => write!(formatter, "invalid image: {reason}"),
            Self::DimensionsTooLarge { width, height } => write!(
                formatter,
                "image is {width}x{height}; each side must be at most {MAX_ATTACHMENT_DIMENSION}"
            ),
            Self::Io(message) => write!(formatter, "could not stage attachment: {message}"),
        }
    }
}

impl std::error::Error for AttachmentError {}

/// Result of a staged upload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedAttachment {
    /// Absolute path the client can type into a terminal.
    pub path: PathBuf,
    pub bytes: u64,
    pub width: u32,
    pub height: u32,
    pub mime: &'static str,
}

/// Counts from one cleanup sweep.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct CleanupReport {
    pub removed_expired: usize,
    pub removed_over_cap: usize,
}

impl CleanupReport {
    #[must_use]
    pub const fn removed(self) -> usize {
        self.removed_expired + self.removed_over_cap
    }
}

/// Owner of the staging directory. Cheap to construct; safe to share.
#[derive(Debug, Clone)]
pub struct AttachmentStager {
    root: PathBuf,
}

impl Default for AttachmentStager {
    fn default() -> Self {
        Self::new(default_root())
    }
}

impl AttachmentStager {
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let root = if root.is_absolute() {
            root
        } else {
            std::env::current_dir()
                .map(|cwd| cwd.join(&root))
                .unwrap_or(root)
        };
        Self { root }
    }

    /// App-owned directory under the OS temp dir.
    #[must_use]
    pub fn default_root() -> PathBuf {
        default_root()
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Validate and persist one raster. Blocking; call from `spawn_blocking`.
    pub fn stage(
        &self,
        bytes: &[u8],
        declared: Option<ImageKind>,
    ) -> Result<StagedAttachment, AttachmentError> {
        if bytes.is_empty() {
            return Err(AttachmentError::Empty);
        }
        if bytes.len() > MAX_ATTACHMENT_BYTES {
            return Err(AttachmentError::TooLarge {
                size: bytes.len(),
                limit: MAX_ATTACHMENT_BYTES,
            });
        }
        let kind = ImageKind::from_magic(bytes).ok_or(AttachmentError::UnsupportedMediaType)?;
        if let Some(declared) = declared {
            if declared != kind {
                return Err(AttachmentError::ContentTypeMismatch);
            }
        }
        // Header-only: `into_dimensions` never decodes the pixel buffer.
        let (width, height) = image_dimensions(bytes)?;
        if width > MAX_ATTACHMENT_DIMENSION || height > MAX_ATTACHMENT_DIMENSION {
            return Err(AttachmentError::DimensionsTooLarge { width, height });
        }

        let root = self.prepare_root()?;
        let file_name = format!(
            "{FILE_PREFIX}{}-{}.{}",
            unix_millis(),
            random_suffix()?,
            kind.extension()
        );
        let path = root.join(file_name);
        write_private(&path, bytes).map_err(|error| AttachmentError::Io(error.to_string()))?;

        // TTL + cap sweep on every stage call. Failure here must not discard the
        // file we just staged; the next call retries.
        if let Err(error) = cleanup_dir(&root, SystemTime::now()) {
            log::warn!(
                target: "se_manager::remote_attachments",
                "operation=cleanup_attachments stable_code=CLEANUP_FAILED error={error}"
            );
        }

        Ok(StagedAttachment {
            path,
            bytes: bytes.len() as u64,
            width,
            height,
            mime: kind.mime(),
        })
    }

    /// Remove expired files and trim the directory to [`MAX_ATTACHMENT_FILES`].
    pub fn cleanup(&self) -> io::Result<CleanupReport> {
        cleanup_dir(&self.root, SystemTime::now())
    }

    fn prepare_root(&self) -> Result<PathBuf, AttachmentError> {
        fs::create_dir_all(&self.root)
            .map_err(|error| AttachmentError::Io(error.to_string()))?;
        #[cfg(unix)]
        set_unix_mode(&self.root, 0o700)?;
        // Return an absolute, symlink-resolved path so the client can type it
        // into a terminal regardless of its own working directory.
        Ok(fs::canonicalize(&self.root).unwrap_or_else(|_| self.root.clone()))
    }
}

/// Default staging root shared by the desktop host and `se-server`.
#[must_use]
pub fn default_root() -> PathBuf {
    std::env::temp_dir().join(DIRECTORY_NAME)
}

/// Run the startup sweep once for the shared default directory.
///
/// Both `web::serve` and the desktop shared-live host enter
/// `web::serve_router`, which calls this exactly once per process.
pub fn cleanup_at_startup() {
    match cleanup_dir(&default_root(), SystemTime::now()) {
        Ok(report) => {
            log::info!(
                target: "se_manager::remote_attachments",
                "operation=cleanup_attachments stable_code=OK removed={}",
                report.removed()
            );
        }
        Err(error) => {
            log::warn!(
                target: "se_manager::remote_attachments",
                "operation=cleanup_attachments stable_code=CLEANUP_FAILED error={error}"
            );
        }
    }
}

fn image_dimensions(bytes: &[u8]) -> Result<(u32, u32), AttachmentError> {
    let reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| AttachmentError::InvalidImage("unreadable header"))?;
    let (width, height) = reader
        .into_dimensions()
        .map_err(|_| AttachmentError::InvalidImage("unreadable dimensions"))?;
    if width == 0 || height == 0 {
        return Err(AttachmentError::InvalidImage("zero dimensions"));
    }
    Ok((width, height))
}

fn unix_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default()
}

fn random_suffix() -> Result<String, AttachmentError> {
    let mut bytes = [0_u8; 8];
    getrandom::getrandom(&mut bytes)
        .map_err(|error| AttachmentError::Io(error.to_string()))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[cfg(unix)]
fn set_unix_mode(path: &Path, mode: u32) -> Result<(), AttachmentError> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = fs::metadata(path)
        .map_err(|error| AttachmentError::Io(error.to_string()))?
        .permissions();
    permissions.set_mode(mode);
    fs::set_permissions(path, permissions).map_err(|error| AttachmentError::Io(error.to_string()))
}

#[cfg(unix)]
fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = file.metadata()?.permissions();
        permissions.set_mode(0o600);
        fs::set_permissions(path, permissions)?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn is_staged_file_name(name: &str) -> bool {
    name.starts_with(FILE_PREFIX)
        && (name.ends_with(".png") || name.ends_with(".jpg") || name.ends_with(".jpeg"))
}

fn cleanup_dir(root: &Path, now: SystemTime) -> io::Result<CleanupReport> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(CleanupReport::default())
        }
        Err(error) => return Err(error),
    };

    let mut report = CleanupReport::default();
    let mut kept: Vec<(PathBuf, SystemTime)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if !is_staged_file_name(&name) {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        let modified = metadata.modified().unwrap_or(now);
        let expired = now
            .duration_since(modified)
            .map(|age| age > MAX_ATTACHMENT_AGE)
            .unwrap_or(false);
        if expired {
            if fs::remove_file(&path).is_ok() {
                report.removed_expired += 1;
            }
        } else {
            kept.push((path, modified));
        }
    }

    // Newest first; filename breaks mtime ties deterministically.
    kept.sort_by(|left, right| {
        right
            .1
            .cmp(&left.1)
            .then_with(|| right.0.cmp(&left.0))
    });
    for (path, _) in kept.into_iter().skip(MAX_ATTACHMENT_FILES) {
        if fs::remove_file(&path).is_ok() {
            report.removed_over_cap += 1;
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    struct TempRoot {
        path: PathBuf,
    }

    impl TempRoot {
        fn new(label: &str) -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "se-manager-attachments-{label}-{}-{nanos}",
                std::process::id()
            ));
            fs::create_dir_all(&path).expect("create temp root");
            Self { path }
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn png(width: u32, height: u32) -> Vec<u8> {
        let image = image::RgbaImage::from_pixel(width, height, image::Rgba([1, 2, 3, 255]));
        let mut out = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image)
            .write_to(&mut out, image::ImageFormat::Png)
            .expect("encode png");
        out.into_inner()
    }

    fn jpeg(width: u32, height: u32) -> Vec<u8> {
        let image = image::RgbImage::from_pixel(width, height, image::Rgb([10, 20, 30]));
        let mut out = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image)
            .write_to(&mut out, image::ImageFormat::Jpeg)
            .expect("encode jpeg");
        out.into_inner()
    }

    #[test]
    fn stages_png_and_jpeg_with_dimensions_and_absolute_path() {
        let temp = TempRoot::new("valid");
        let stager = AttachmentStager::new(&temp.path);

        let png_bytes = png(3, 2);
        let staged = stager
            .stage(&png_bytes, Some(ImageKind::Png))
            .expect("stage png");
        assert!(staged.path.is_absolute(), "{:?}", staged.path);
        assert!(staged.path.starts_with(fs::canonicalize(&temp.path).unwrap()));
        assert!(staged.path.to_string_lossy().contains("/se-shot-"));
        assert_eq!(staged.path.extension().and_then(|e| e.to_str()), Some("png"));
        assert_eq!((staged.width, staged.height), (3, 2));
        assert_eq!(staged.bytes, png_bytes.len() as u64);
        assert_eq!(staged.mime, "image/png");
        assert_eq!(fs::read(&staged.path).unwrap(), png_bytes);

        let jpeg_bytes = jpeg(4, 5);
        let staged = stager
            .stage(&jpeg_bytes, Some(ImageKind::Jpeg))
            .expect("stage jpeg");
        assert_eq!(staged.path.extension().and_then(|e| e.to_str()), Some("jpg"));
        assert_eq!((staged.width, staged.height), (4, 5));
        assert_eq!(staged.mime, "image/jpeg");
    }

    #[test]
    fn rejects_empty_wrong_magic_and_content_type_mismatch() {
        let temp = TempRoot::new("magic");
        let stager = AttachmentStager::new(&temp.path);
        assert_eq!(stager.stage(&[], None), Err(AttachmentError::Empty));
        assert_eq!(
            stager.stage(b"GIF89a\x01\x00\x01\x00", None),
            Err(AttachmentError::UnsupportedMediaType)
        );
        let png_bytes = png(1, 1);
        assert_eq!(
            stager.stage(&png_bytes, Some(ImageKind::Jpeg)),
            Err(AttachmentError::ContentTypeMismatch)
        );
        assert!(fs::read_dir(&temp.path).unwrap().next().is_none());
    }

    #[test]
    fn rejects_oversize_before_writing() {
        let temp = TempRoot::new("oversize");
        let stager = AttachmentStager::new(&temp.path);
        let mut bytes = vec![0_u8; MAX_ATTACHMENT_BYTES + 1];
        bytes[..8].copy_from_slice(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);
        assert_eq!(
            stager.stage(&bytes, Some(ImageKind::Png)),
            Err(AttachmentError::TooLarge {
                size: MAX_ATTACHMENT_BYTES + 1,
                limit: MAX_ATTACHMENT_BYTES,
            })
        );
        assert!(fs::read_dir(&temp.path).unwrap().next().is_none());
    }

    #[test]
    fn rejects_dimensions_over_cap_from_header_only() {
        let temp = TempRoot::new("dimensions");
        let stager = AttachmentStager::new(&temp.path);
        let bytes = png(MAX_ATTACHMENT_DIMENSION + 1, 1);
        assert_eq!(
            stager.stage(&bytes, Some(ImageKind::Png)),
            Err(AttachmentError::DimensionsTooLarge {
                width: MAX_ATTACHMENT_DIMENSION + 1,
                height: 1,
            })
        );
        assert!(fs::read_dir(&temp.path).unwrap().next().is_none());
    }

    #[test]
    fn cleanup_removes_expired_files() {
        let temp = TempRoot::new("ttl");
        let stager = AttachmentStager::new(&temp.path);
        let staged = stager.stage(&png(1, 1), None).expect("stage");
        let later = SystemTime::now() + MAX_ATTACHMENT_AGE + Duration::from_secs(1);
        let report = cleanup_dir(&temp.path, later).expect("cleanup");
        assert_eq!(report.removed_expired, 1);
        assert!(!staged.path.exists());
    }

    #[test]
    fn cleanup_keeps_at_most_max_files() {
        let temp = TempRoot::new("cap");
        for index in 0..(MAX_ATTACHMENT_FILES + 7) {
            fs::write(
                temp.path.join(format!("{FILE_PREFIX}{index}.png")),
                b"not-a-real-image",
            )
            .expect("write fixture");
        }
        let stager = AttachmentStager::new(&temp.path);
        stager.cleanup().expect("cleanup");
        let remaining = fs::read_dir(&temp.path).unwrap().count();
        assert_eq!(remaining, MAX_ATTACHMENT_FILES);
    }

    #[test]
    fn cleanup_ignores_unrelated_files() {
        let temp = TempRoot::new("unrelated");
        let unrelated = temp.path.join("notes.txt");
        fs::write(&unrelated, b"keep me").unwrap();
        cleanup_dir(&temp.path, SystemTime::now()).expect("cleanup");
        assert!(unrelated.exists());
    }

    #[cfg(unix)]
    #[test]
    fn unix_permissions_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let temp = TempRoot::new("perms");
        let stager = AttachmentStager::new(&temp.path);
        let staged = stager.stage(&png(1, 1), None).expect("stage");
        let dir_mode = fs::metadata(&temp.path).unwrap().permissions().mode() & 0o777;
        let file_mode = fs::metadata(&staged.path).unwrap().permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o700);
        assert_eq!(file_mode, 0o600);
    }

    #[test]
    fn content_type_parsing_is_strict() {
        assert_eq!(ImageKind::from_content_type("image/png"), Some(ImageKind::Png));
        assert_eq!(
            ImageKind::from_content_type("Image/JPEG; charset=binary"),
            Some(ImageKind::Jpeg)
        );
        assert_eq!(ImageKind::from_content_type("image/gif"), None);
        assert_eq!(ImageKind::from_content_type("text/plain"), None);
    }
}
