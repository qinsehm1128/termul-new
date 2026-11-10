//! Blocking remote desktop engine. Transport/auth policy belongs to the host lane.
//!
//! `origin_*` are global desktop points; dimensions are physical pixels. macOS
//! uses Quartz points, while Windows injection translates to native physical
//! coordinates at its boundary. No pixels or input text are written to logs.
#![allow(dead_code, unused_imports)] // Consumed by the separately delivered host adapters.

mod codec;
mod h264;
mod input;
mod stream;

pub use codec::*;
pub use h264::*;
pub use input::*;
pub use stream::*;

use image::{DynamicImage, RgbaImage};
use serde::{Deserialize, Serialize};
use std::io::Cursor;
use std::time::Instant;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DisplayInfo {
    /// Native monitor identity, never an enumeration index.
    pub id: String,
    pub name: String,
    pub pixel_width: u32,
    pub pixel_height: u32,
    pub scale: f64,
    pub origin_x: i32,
    pub origin_y: i32,
    pub is_primary: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScreenError {
    Unsupported,
    ScreenPermissionRequired,
    DisplayNotFound,
    CaptureFailed(String),
    InputPermissionRequired,
    InputFailed(String),
}

impl ScreenError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Unsupported => "SCREEN_UNSUPPORTED",
            Self::ScreenPermissionRequired => "SCREEN_PERMISSION_REQUIRED",
            Self::DisplayNotFound => "DISPLAY_NOT_FOUND",
            Self::CaptureFailed(_) => "CAPTURE_FAILED",
            Self::InputPermissionRequired => "INPUT_PERMISSION_REQUIRED",
            Self::InputFailed(_) => "INPUT_FAILED",
        }
    }
}

impl std::fmt::Display for ScreenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.code())?;
        if let Self::CaptureFailed(detail) | Self::InputFailed(detail) = self {
            write!(f, ": {detail}")?;
        }
        Ok(())
    }
}

impl std::error::Error for ScreenError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFormat {
    Jpeg { quality: u8 },
    Png,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaptureOptions {
    pub max_width: u32,
    pub format: ImageFormat,
}

impl Default for CaptureOptions {
    fn default() -> Self {
        Self {
            max_width: 1920,
            format: ImageFormat::Jpeg { quality: 80 },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedImage {
    pub bytes: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub mime: &'static str,
}

/// No native capture dependencies are linked on Linux, including se-server.
/// A missing GUI session (headless/locked macOS) is not supported.
pub fn supported() -> bool {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        #[cfg(target_os = "macos")]
        if !crate::macos_permissions::desktop_session_active() {
            return false;
        }
        xcap::Monitor::all().is_ok_and(|monitors| !monitors.is_empty())
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        false
    }
}

pub fn list_displays() -> Result<Vec<DisplayInfo>, ScreenError> {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        #[cfg(target_os = "macos")]
        if !crate::macos_permissions::desktop_session_active() {
            return Err(ScreenError::Unsupported);
        }
        let monitors = xcap::Monitor::all().map_err(capture_error)?;
        if monitors.is_empty() {
            return Err(ScreenError::Unsupported);
        }
        monitors.iter().map(display_info).collect()
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        Err(ScreenError::Unsupported)
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn capture_error(error: impl std::fmt::Display) -> ScreenError {
    ScreenError::CaptureFailed(error.to_string())
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn display_info(monitor: &xcap::Monitor) -> Result<DisplayInfo, ScreenError> {
    let name = monitor.name().map_err(capture_error)?;
    let scale = f64::from(monitor.scale_factor().map_err(capture_error)?);
    if !scale.is_finite() || scale <= 0.0 {
        return Err(ScreenError::CaptureFailed("invalid monitor scale".into()));
    }
    let width = monitor.width().map_err(capture_error)?;
    let height = monitor.height().map_err(capture_error)?;
    // xcap's macOS CGDisplayBounds uses points; GDI dimensions are pixels.
    #[cfg(target_os = "macos")]
    let (width, height) = (
        (f64::from(width) * scale).round() as u32,
        (f64::from(height) * scale).round() as u32,
    );
    #[cfg(target_os = "macos")]
    let id = format!("macos:{}", monitor.id().map_err(capture_error)?);
    // Windows Monitor::id is derived from a pointer; the device name is stable
    // across enumerations and does not truncate a 64-bit HMONITOR.
    #[cfg(target_os = "windows")]
    let id = format!("windows:{name}");
    Ok(DisplayInfo {
        id,
        name,
        pixel_width: width,
        pixel_height: height,
        scale,
        origin_x: monitor.x().map_err(capture_error)?,
        origin_y: monitor.y().map_err(capture_error)?,
        is_primary: monitor.is_primary().map_err(capture_error)?,
    })
}

/// Capture, resize and encode synchronously. Use `capture_async` in async hosts.
pub fn capture(display_id: &str, opts: CaptureOptions) -> Result<EncodedImage, ScreenError> {
    let started = Instant::now();
    let result = (|| {
        if !cfg!(any(target_os = "macos", target_os = "windows")) {
            return Err(ScreenError::Unsupported);
        }
        validate_options(opts)?;
        let frame = capture_raw(display_id, opts.max_width)?;
        encode_image(&frame, opts.format)
    })();
    log::info!(
        "operation=desktop_capture stable_code={} duration_ms={}",
        result
            .as_ref()
            .map_or_else(|error: &ScreenError| error.code(), |_| "OK"),
        started.elapsed().as_millis()
    );
    result
}

fn validate_options(opts: CaptureOptions) -> Result<(), ScreenError> {
    if opts.max_width == 0 || opts.max_width > 8192 {
        return Err(ScreenError::CaptureFailed(
            "max_width must be in 1..=8192".into(),
        ));
    }
    if matches!(
        opts.format,
        ImageFormat::Jpeg {
            quality: 0 | 101..=255
        }
    ) {
        return Err(ScreenError::CaptureFailed(
            "JPEG quality must be in 1..=100".into(),
        ));
    }
    Ok(())
}

/// Internal raw-frame path shared with the streaming worker; no async runtime
/// thread is blocked. Every call re-enumerates the requested native monitor.
pub(super) fn capture_raw(display_id: &str, max_width: u32) -> Result<RgbaImage, ScreenError> {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        if !(1..=8192).contains(&max_width) {
            return Err(ScreenError::CaptureFailed("invalid max_width".into()));
        }
        #[cfg(target_os = "macos")]
        {
            if !crate::macos_permissions::desktop_session_active() {
                return Err(ScreenError::Unsupported);
            }
            if !crate::macos_permissions::ensure_desktop_capture_access() {
                return Err(ScreenError::ScreenPermissionRequired);
            }
        }
        let monitors = xcap::Monitor::all().map_err(capture_error)?;
        if monitors.is_empty() {
            return Err(ScreenError::Unsupported);
        }
        for monitor in monitors {
            if display_info(&monitor)?.id == display_id {
                let frame = monitor.capture_image().map_err(capture_error)?;
                return downscale(frame, max_width);
            }
        }
        Err(ScreenError::DisplayNotFound)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = (display_id, max_width);
        Err(ScreenError::Unsupported)
    }
}

fn downscale(frame: RgbaImage, max_width: u32) -> Result<RgbaImage, ScreenError> {
    if frame.width() == 0 || frame.height() == 0 || max_width == 0 {
        return Err(ScreenError::CaptureFailed(
            "empty image or invalid width".into(),
        ));
    }
    if frame.width() <= max_width {
        return Ok(frame);
    }
    let height = ((u64::from(frame.height()) * u64::from(max_width)) / u64::from(frame.width()))
        .max(1) as u32;
    Ok(image::imageops::resize(
        &frame,
        max_width,
        height,
        image::imageops::FilterType::Triangle,
    ))
}

fn encode_image(frame: &RgbaImage, format: ImageFormat) -> Result<EncodedImage, ScreenError> {
    let mut bytes = Vec::new();
    let mime = match format {
        ImageFormat::Jpeg { quality } => {
            let rgb = DynamicImage::ImageRgba8(frame.clone()).into_rgb8();
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, quality)
                .encode_image(&rgb)
                .map_err(|e| ScreenError::CaptureFailed(e.to_string()))?;
            "image/jpeg"
        }
        ImageFormat::Png => {
            DynamicImage::ImageRgba8(frame.clone())
                .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
                .map_err(|e| ScreenError::CaptureFailed(e.to_string()))?;
            "image/png"
        }
    };
    Ok(EncodedImage {
        bytes,
        width: frame.width(),
        height: frame.height(),
        mime,
    })
}

pub async fn capture_async(
    display_id: String,
    opts: CaptureOptions,
) -> Result<EncodedImage, ScreenError> {
    tokio::task::spawn_blocking(move || capture(&display_id, opts))
        .await
        .map_err(|e| ScreenError::CaptureFailed(format!("capture worker: {e}")))?
}

pub async fn list_displays_async() -> Result<Vec<DisplayInfo>, ScreenError> {
    tokio::task::spawn_blocking(list_displays)
        .await
        .map_err(|e| ScreenError::CaptureFailed(format!("display worker: {e}")))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_stable_codes() {
        assert_eq!(CaptureOptions::default().max_width, 1920);
        for (error, code) in [
            (ScreenError::Unsupported, "SCREEN_UNSUPPORTED"),
            (
                ScreenError::ScreenPermissionRequired,
                "SCREEN_PERMISSION_REQUIRED",
            ),
            (ScreenError::DisplayNotFound, "DISPLAY_NOT_FOUND"),
            (ScreenError::CaptureFailed("test".into()), "CAPTURE_FAILED"),
            (
                ScreenError::InputPermissionRequired,
                "INPUT_PERMISSION_REQUIRED",
            ),
            (ScreenError::InputFailed("test".into()), "INPUT_FAILED"),
        ] {
            assert_eq!(error.code(), code);
        }
    }

    #[test]
    fn resize_preserves_aspect_and_never_upscales() {
        let frame = RgbaImage::new(400, 200);
        let small = downscale(frame, 100).unwrap();
        assert_eq!(small.dimensions(), (100, 50));
        assert_eq!(downscale(small, 1920).unwrap().dimensions(), (100, 50));
        assert!(downscale(RgbaImage::new(0, 0), 100).is_err());
    }

    #[test]
    fn image_encoders_produce_decodable_dimensions() {
        for format in [ImageFormat::Jpeg { quality: 80 }, ImageFormat::Png] {
            let encoded = encode_image(&RgbaImage::new(32, 16), format).unwrap();
            assert_eq!(image::load_from_memory(&encoded.bytes).unwrap().width(), 32);
            assert_eq!(encoded.height, 16);
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    #[tokio::test]
    async fn unsupported_platform_is_explicit() {
        assert!(!supported());
        assert_eq!(list_displays().unwrap_err().code(), "SCREEN_UNSUPPORTED");
        assert_eq!(
            capture("unknown", CaptureOptions::default())
                .unwrap_err()
                .code(),
            "SCREEN_UNSUPPORTED"
        );
        assert_eq!(
            capture_async("unknown".into(), CaptureOptions::default())
                .await
                .unwrap_err()
                .code(),
            "SCREEN_UNSUPPORTED"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "real Screen Recording TCC check; requires an unlocked macOS desktop"]
    fn real_capture_main_display() {
        let displays = list_displays().expect("real macOS GUI display enumeration");
        let main = displays
            .iter()
            .find(|d| d.is_primary)
            .expect("primary display");
        let image = capture(&main.id, CaptureOptions::default());
        match image {
            Ok(image) => {
                assert!(!image.bytes.is_empty());
                assert!(image.width > 0 && image.width <= 1920);
                println!(
                    "real capture OK: {}x{}, {} bytes, {}",
                    image.width,
                    image.height,
                    image.bytes.len(),
                    image.mime
                );
            }
            Err(error) => panic!(
                "real capture failed: {} (grant Screen Recording to the test host and retry)",
                error.code()
            ),
        }
    }
}
