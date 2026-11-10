//! H.264 (AVC) encoding through macOS VideoToolbox.
//!
//! This module only turns an `RgbaImage` into an AVCC access unit plus SPS/PPS.
//! Wire framing, keyframe policy and packet validation live in [`super::codec`].
//! Hardware encoding is preferred but never required: when no
//! `VTCompressionSession` can be created the host advertises `codecs: ["jpeg"]`
//! and the JPEG path is unchanged. Frame bytes are never logged.
//!
//! Windows/Linux compile without VideoToolbox: every entry point is a stub that
//! reports the codec as unavailable.

use std::fmt;

/// One encoded H.264 access unit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedH264 {
    /// True for an IDR sample; parameter sets are present only on keyframes.
    pub keyframe: bool,
    pub width: u32,
    pub height: u32,
    /// Raw SPS NAL without start code (keyframes only).
    pub sps: Vec<u8>,
    /// Raw PPS NAL without start code (keyframes only).
    pub pps: Vec<u8>,
    /// AVCC sample: 4-byte big-endian NAL lengths, no start codes.
    pub sample: Vec<u8>,
}

impl EncodedH264 {
    /// Packet payload for [`super::codec::H264Frame`]: SPS + PPS + sample.
    pub fn payload(&self) -> Vec<u8> {
        let mut payload =
            Vec::with_capacity(self.sps.len() + self.pps.len() + self.sample.len());
        payload.extend_from_slice(&self.sps);
        payload.extend_from_slice(&self.pps);
        payload.extend_from_slice(&self.sample);
        payload
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum H264Error {
    /// No usable VideoToolbox encoder on this platform or machine.
    Unavailable,
    /// The session rejected this frame; the stream reports the boundary.
    Rejected(String),
}

impl H264Error {
    /// Log-safe reason: never contains frame bytes or tokens.
    pub fn reason(&self) -> String {
        match self {
            Self::Unavailable => "videotoolbox_unavailable".to_owned(),
            Self::Rejected(detail) => format!("rejected:{detail}"),
        }
    }
}

impl fmt::Display for H264Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.reason())
    }
}

impl std::error::Error for H264Error {}

/// Hardware/software encoder availability, probed lazily and cached.
///
/// The probe creates and tears down a real session, which is the only reliable
/// signal: framework presence alone does not guarantee a usable encoder.
pub fn available() -> bool {
    #[cfg(test)]
    if let Some(forced) = TEST_AVAILABILITY.with(std::cell::Cell::get) {
        return forced;
    }
    #[cfg(target_os = "macos")]
    {
        static AVAILABLE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *AVAILABLE.get_or_init(|| match macos::probe() {
            Ok(()) => true,
            Err(reason) => {
                log::warn!("desktop_h264_unavailable reason={reason}");
                false
            }
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

#[cfg(test)]
thread_local! {
    static TEST_AVAILABILITY: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

/// Test-only override for the cached probe. Session starts run on the caller's
/// thread, so a thread-local value is enough and no parallel test is affected.
#[cfg(test)]
pub fn force_availability_for_tests(value: Option<bool>) -> Option<bool> {
    TEST_AVAILABILITY.with(|slot| slot.replace(value))
}

/// Value advertised by `GET /remote/desktop/info`.
pub fn available_codecs() -> Vec<&'static str> {
    if available() {
        vec!["h264", "jpeg"]
    } else {
        vec!["jpeg"]
    }
}

/// Serializes VideoToolbox sessions across the lib test binary: parallel
/// encoders share the hardware and would make the throughput assertion flaky.
#[cfg(test)]
static TEST_ENCODER_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
pub(crate) fn lock_test_encoder() -> std::sync::MutexGuard<'static, ()> {
    TEST_ENCODER_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// H.264 requires even dimensions; odd capture sizes lose the last column/row.
pub fn even_dimension(value: u32) -> u32 {
    value & !1
}

/// True when a capture of `width`x`height` can be encoded at all.
pub fn can_encode(width: u32, height: u32) -> bool {
    even_dimension(width) > 0 && even_dimension(height) > 0
}

pub struct H264Encoder {
    #[cfg(target_os = "macos")]
    inner: macos::Encoder,
    width: u32,
    height: u32,
}

impl H264Encoder {
    /// Creates a session at the given capture size (cropped to even dimensions).
    pub fn new(
        width: u32,
        height: u32,
        bitrate_kbps: u32,
        fps: u32,
    ) -> Result<Self, H264Error> {
        let width = even_dimension(width);
        let height = even_dimension(height);
        if width == 0 || height == 0 {
            return Err(H264Error::Rejected("empty_dimensions".into()));
        }
        #[cfg(target_os = "macos")]
        {
            let inner = macos::Encoder::new(width, height, bitrate_kbps, fps.max(1))?;
            Ok(Self {
                inner,
                width,
                height,
            })
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (bitrate_kbps, fps);
            Err(H264Error::Unavailable)
        }
    }

    /// Even dimensions this session encodes; callers recreate on changes.
    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Encodes one frame. `force_keyframe` requests an IDR. Returns `None` when
    /// VideoToolbox dropped the frame (for example while it reconfigures).
    pub fn encode(
        &mut self,
        image: &image::RgbaImage,
        force_keyframe: bool,
    ) -> Result<Option<EncodedH264>, H264Error> {
        let (width, height) = image.dimensions();
        if width < self.width || height < self.height {
            return Err(H264Error::Rejected("capture_smaller_than_session".into()));
        }
        #[cfg(target_os = "macos")]
        {
            self.inner.encode(image, self.width, self.height, force_keyframe)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (image, force_keyframe);
            Err(H264Error::Unavailable)
        }
    }
}

/// RGB A row copy used by [`macos`]; pure so it is testable with a synthetic
/// stride. `source_stride` and `destination_stride` are bytes per row and
/// must both be at least `width * 4`; destination padding is left untouched
/// so a padded row never overwrites the previous row's tail.
fn copy_rgba_to_bgra(
    source: &[u8],
    source_stride: usize,
    destination: &mut [u8],
    destination_stride: usize,
    width: usize,
    height: usize,
) -> Result<(), H264Error> {
    let row_bytes = width
        .checked_mul(4)
        .ok_or_else(|| H264Error::Rejected("invalid_pixel_buffer_layout".into()))?;
    if width == 0 || height == 0 || row_bytes == 0 {
        return Err(H264Error::Rejected("invalid_pixel_buffer_layout".into()));
    }
    if source_stride < row_bytes || destination_stride < row_bytes {
        return Err(H264Error::Rejected("invalid_pixel_buffer_layout".into()));
    }
    let source_bytes = (height - 1)
        .checked_mul(source_stride)
        .and_then(|n| n.checked_add(row_bytes))
        .ok_or_else(|| H264Error::Rejected("invalid_pixel_buffer_layout".into()))?;
    let destination_bytes = (height - 1)
        .checked_mul(destination_stride)
        .and_then(|n| n.checked_add(row_bytes))
        .ok_or_else(|| H264Error::Rejected("invalid_pixel_buffer_layout".into()))?;
    if source.len() < source_bytes || destination.len() < destination_bytes {
        return Err(H264Error::Rejected("invalid_pixel_buffer_layout".into()));
    }
    for row in 0..height {
        let source_start = row * source_stride;
        let destination_start = row * destination_stride;
        let src = &source[source_start..source_start + row_bytes];
        let dst = &mut destination[destination_start..destination_start + row_bytes];
        for (pixel, out) in src.chunks_exact(4).zip(dst.chunks_exact_mut(4)) {
            // RGBA -> BGRA.
            out[0] = pixel[2];
            out[1] = pixel[1];
            out[2] = pixel[0];
            out[3] = pixel[3];
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
mod macos {
    use super::{EncodedH264, H264Error};
    use image::RgbaImage;
    use objc2_core_foundation::{
        CFBoolean, CFDictionary, CFNumber, CFRetained, CFString, Type,
    };
    use objc2_core_media::{
        kCMSampleAttachmentKey_NotSync, CMBlockBuffer, CMFormatDescription, CMSampleBuffer,
    };
    use objc2_core_video::{kCVPixelFormatType_32BGRA, CVPixelBuffer, CVPixelBufferLockFlags};
    use std::{
        ffi::c_void,
        ptr::{self, NonNull},
        sync::{
            atomic::{AtomicU64, Ordering},
            Mutex,
        },
    };

    type OSStatus = i32;
    type CFTypeRef = *const c_void;
    type VTCompressionSessionRef = *mut c_void;
    type CMSampleBufferRef = *mut c_void;
    type CMImageBufferRef = *const c_void;

    const K_CM_VIDEO_CODEC_TYPE_H264: u32 = 0x6176_6331; // 'avc1'
    /// Hard cap for one access unit; a 4K keyframe is well below this.
    const MAX_SAMPLE_BYTES: usize = 16 * 1024 * 1024;
    const AVCC_HEADER_BYTES: i32 = 4;

    /// C `CMTime` layout (value/timescale = seconds).
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct CMTime {
        value: i64,
        timescale: i32,
        flags: u32,
        epoch: i64,
    }

    impl CMTime {
        fn seconds(value: i64, timescale: i32) -> Self {
            Self {
                value,
                timescale,
                flags: 1, // kCMTimeFlags_Valid
                epoch: 0,
            }
        }

        fn invalid() -> Self {
            Self::default()
        }
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFRetain(cf: CFTypeRef) -> CFTypeRef;
        fn CFRelease(cf: CFTypeRef);
        fn CFArrayGetCount(array: CFTypeRef) -> isize;
        fn CFArrayGetValueAtIndex(array: CFTypeRef, index: isize) -> *const c_void;
        fn CFDictionaryContainsKey(dict: CFTypeRef, key: CFTypeRef) -> u8;
    }

    #[link(name = "VideoToolbox", kind = "framework")]
    extern "C" {
        static kVTCompressionPropertyKey_RealTime: *const CFString;
        static kVTCompressionPropertyKey_AllowFrameReordering: *const CFString;
        static kVTCompressionPropertyKey_ProfileLevel: *const CFString;
        static kVTCompressionPropertyKey_AverageBitRate: *const CFString;
        static kVTCompressionPropertyKey_MaxKeyFrameIntervalDuration: *const CFString;
        static kVTCompressionPropertyKey_ExpectedFrameRate: *const CFString;
        /// Encoder-specification key (not a session property): hardware first.
        static kVTVideoEncoderSpecification_EnableHardwareAcceleratedVideoEncoder: *const CFString;
        static kVTProfileLevel_H264_High_AutoLevel: *const CFString;
        static kVTProfileLevel_H264_Main_AutoLevel: *const CFString;
        static kVTEncodeFrameOptionKey_ForceKeyFrame: *const CFString;

        fn VTCompressionSessionCreate(
            allocator: CFTypeRef,
            width: i32,
            height: i32,
            codec_type: u32,
            encoder_specification: CFTypeRef,
            source_image_buffer_attributes: CFTypeRef,
            compressed_data_allocator: CFTypeRef,
            output_callback: Option<OutputCallback>,
            output_callback_ref_con: *mut c_void,
            compression_session_out: *mut VTCompressionSessionRef,
        ) -> OSStatus;
        fn VTCompressionSessionPrepareToEncodeFrames(
            session: VTCompressionSessionRef,
        ) -> OSStatus;
        fn VTSessionSetProperty(
            session: VTCompressionSessionRef,
            key: CFTypeRef,
            value: CFTypeRef,
        ) -> OSStatus;
        fn VTCompressionSessionEncodeFrame(
            session: VTCompressionSessionRef,
            image_buffer: *mut CVPixelBuffer,
            presentation_time_stamp: CMTime,
            duration: CMTime,
            frame_properties: CFTypeRef,
            source_frame_ref_con: *mut c_void,
            info_flags_out: *mut u32,
        ) -> OSStatus;
        fn VTCompressionSessionCompleteFrames(
            session: VTCompressionSessionRef,
            complete_until_presentation_time_stamp: CMTime,
        ) -> OSStatus;
        fn VTCompressionSessionInvalidate(session: VTCompressionSessionRef);
    }

    type OutputCallback = unsafe extern "C" fn(
        output_callback_ref_con: *mut c_void,
        source_frame_ref_con: *mut c_void,
        status: OSStatus,
        info_flags: u32,
        sample_buffer: CMSampleBufferRef,
    );

    /// Retained sample buffers handed from the VT callback to the encoder thread.
    struct CallbackCtx {
        samples: Mutex<Vec<*mut c_void>>,
        dropped: AtomicU64,
    }

    // The raw pointers are CF objects retained by us; access is mutex-guarded.
    unsafe impl Send for CallbackCtx {}
    unsafe impl Sync for CallbackCtx {}

    unsafe extern "C" fn output_callback(
        ref_con: *mut c_void,
        _source: *mut c_void,
        status: OSStatus,
        _info_flags: u32,
        sample_buffer: CMSampleBufferRef,
    ) {
        if ref_con.is_null() {
            return;
        }
        let ctx = unsafe { &*(ref_con as *const CallbackCtx) };
        if status != 0 || sample_buffer.is_null() {
            ctx.dropped.fetch_add(1, Ordering::Relaxed);
            return;
        }
        // Retain before returning: VT owns the buffer only for this callback.
        unsafe { CFRetain(sample_buffer as CFTypeRef) };
        let mut queue = ctx
            .samples
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        queue.push(sample_buffer);
    }

    pub(super) struct Encoder {
        session: VTCompressionSessionRef,
        ctx: Box<CallbackCtx>,
        fps: u32,
        frame_index: i64,
    }

    impl Encoder {
        pub(super) fn new(
            width: u32,
            height: u32,
            bitrate_kbps: u32,
            fps: u32,
        ) -> Result<Self, H264Error> {
            let mut ctx = Box::new(CallbackCtx {
                samples: Mutex::new(Vec::new()),
                dropped: AtomicU64::new(0),
            });
            // Prefer hardware, but never require it: a NULL/empty specification
            // would also let VT pick the software encoder.
            let spec_key: &CFString =
                unsafe { &*kVTVideoEncoderSpecification_EnableHardwareAcceleratedVideoEncoder };
            let specification =
                CFDictionary::from_slices(&[spec_key], &[CFBoolean::new(true)]);
            let specification_ref =
                CFRetained::as_ptr(&specification).as_ptr() as CFTypeRef;
            let mut session: VTCompressionSessionRef = ptr::null_mut();
            let status = unsafe {
                VTCompressionSessionCreate(
                    ptr::null(),
                    width as i32,
                    height as i32,
                    K_CM_VIDEO_CODEC_TYPE_H264,
                    specification_ref,
                    ptr::null(),
                    ptr::null(),
                    Some(output_callback),
                    (&mut *ctx) as *mut CallbackCtx as *mut c_void,
                    &mut session,
                )
            };
            drop(specification);
            if status != 0 || session.is_null() {
                return Err(H264Error::Rejected(format!(
                    "VTCompressionSessionCreate(status={status})"
                )));
            }
            let mut encoder = Self {
                session,
                ctx,
                fps,
                frame_index: 0,
            };
            if let Err(error) = encoder.configure(bitrate_kbps) {
                // Explicit teardown nulls the session so Drop does not release it twice.
                encoder.drop_session();
                return Err(error);
            }
            Ok(encoder)
        }

        fn configure(&self, bitrate_kbps: u32) -> Result<(), H264Error> {
            let mut failures = Vec::new();
            let mut set = |key: *const CFString, value: CFTypeRef, name: &str| {
                if key.is_null() {
                    return;
                }
                let status = unsafe { VTSessionSetProperty(self.session, key as CFTypeRef, value) };
                if status != 0 {
                    failures.push(format!("{name}:{status}"));
                }
            };
            let boolean = |value: bool| -> CFTypeRef {
                CFBoolean::new(value) as *const CFBoolean as CFTypeRef
            };
            set(
                unsafe { kVTCompressionPropertyKey_RealTime },
                boolean(true),
                "RealTime",
            );
            set(
                unsafe { kVTCompressionPropertyKey_AllowFrameReordering },
                boolean(false),
                "AllowFrameReordering",
            );
            let bitrate = CFNumber::new_i32((bitrate_kbps.max(1) * 1000) as i32);
            set(
                unsafe { kVTCompressionPropertyKey_AverageBitRate },
                CFRetained::as_ptr(&bitrate).as_ptr() as CFTypeRef,
                "AverageBitRate",
            );
            let fps = CFNumber::new_i32(self.fps.max(1) as i32);
            set(
                unsafe { kVTCompressionPropertyKey_ExpectedFrameRate },
                CFRetained::as_ptr(&fps).as_ptr() as CFTypeRef,
                "ExpectedFrameRate",
            );
            let interval = CFNumber::new_i32(
                super::super::codec::H264_KEYFRAME_INTERVAL_SECONDS as i32,
            );
            set(
                unsafe { kVTCompressionPropertyKey_MaxKeyFrameIntervalDuration },
                CFRetained::as_ptr(&interval).as_ptr() as CFTypeRef,
                "MaxKeyFrameIntervalDuration",
            );
            // High profile first; Main is the documented fallback on older GPUs.
            let high = unsafe { kVTProfileLevel_H264_High_AutoLevel };
            if !high.is_null() {
                let status = unsafe {
                    VTSessionSetProperty(
                        self.session,
                        kVTCompressionPropertyKey_ProfileLevel as CFTypeRef,
                        high as CFTypeRef,
                    )
                };
                if status != 0 {
                    let main = unsafe { kVTProfileLevel_H264_Main_AutoLevel };
                    if !main.is_null() {
                        let status = unsafe {
                            VTSessionSetProperty(
                                self.session,
                                kVTCompressionPropertyKey_ProfileLevel as CFTypeRef,
                                main as CFTypeRef,
                            )
                        };
                        if status != 0 {
                            failures.push(format!("ProfileLevel:{status}"));
                        }
                    }
                }
            }
            // Rate control and profile hints are best-effort. RealTime and
            // frame reordering are mandatory for a low-latency stream.
            if let Some(fatal) = failures
                .iter()
                .find(|entry| entry.starts_with("RealTime") || entry.starts_with("AllowFrameReordering"))
            {
                return Err(H264Error::Rejected(fatal.clone()));
            }
            if !failures.is_empty() {
                log::warn!(
                    "desktop_h264_property_fallback properties={}",
                    failures.join(",")
                );
            }
            let status = unsafe { VTCompressionSessionPrepareToEncodeFrames(self.session) };
            if status != 0 {
                return Err(H264Error::Rejected(format!(
                    "VTCompressionSessionPrepareToEncodeFrames(status={status})"
                )));
            }
            Ok(())
        }

        pub(super) fn encode(
            &mut self,
            image: &RgbaImage,
            width: u32,
            height: u32,
            force_keyframe: bool,
        ) -> Result<Option<EncodedH264>, H264Error> {
            let pixel_buffer = copy_to_pixel_buffer(image, width, height)?;
            let pts = CMTime::seconds(
                self.frame_index * 1_000 / i64::from(self.fps.max(1)),
                1_000,
            );
            let duration = CMTime::seconds(1_000 / i64::from(self.fps.max(1)), 1_000);
            self.frame_index = self.frame_index.wrapping_add(1);

            let frame_properties = if force_keyframe {
                Some(force_keyframe_properties())
            } else {
                None
            };
            let frame_properties_ref = frame_properties
                .as_ref()
                .map_or(ptr::null(), |dict| {
                    CFRetained::as_ptr(dict).as_ptr() as CFTypeRef
                });
            let mut info_flags = 0u32;
            let status = unsafe {
                VTCompressionSessionEncodeFrame(
                    self.session,
                    pixel_buffer.as_ptr(),
                    pts,
                    duration,
                    frame_properties_ref,
                    ptr::null_mut(),
                    &mut info_flags,
                )
            };
            drop(pixel_buffer);
            if status != 0 {
                return Err(H264Error::Rejected(format!(
                    "VTCompressionSessionEncodeFrame(status={status})"
                )));
            }
            // Real-time sessions buffer one frame; completion forces output so
            // the capture thread owns the ordering (no async callback races).
            let status =
                unsafe { VTCompressionSessionCompleteFrames(self.session, CMTime::invalid()) };
            if status != 0 {
                return Err(H264Error::Rejected(format!(
                    "VTCompressionSessionCompleteFrames(status={status})"
                )));
            }
            let samples: Vec<*mut c_void> = {
                let mut queue = self
                    .ctx
                    .samples
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                std::mem::take(&mut *queue)
            };
            let mut encoded = None;
            for sample in samples {
                let result = unsafe { extract(sample) };
                unsafe { CFRelease(sample as CFTypeRef) };
                match result? {
                    Some(mut frame) => {
                        frame.width = width;
                        frame.height = height;
                        encoded = Some(frame);
                    }
                    None => {}
                }
            }
            Ok(encoded)
        }

        /// Drop order contract: flush every queued frame through the output
        /// callback while the session AND the `CallbackCtx` refcon are still
        /// valid, then invalidate, then release the session, then drain any
        /// retained samples. `VTCompressionSessionCompleteFrames` blocks until
        /// all pending callbacks have run, so no callback can outlive `ctx`.
        fn drop_session(&mut self) {
            if !self.session.is_null() {
                let session = self.session;
                self.session = ptr::null_mut();
                unsafe {
                    let status = VTCompressionSessionCompleteFrames(session, CMTime::invalid());
                    if status != 0 {
                        log::warn!("desktop_h264_flush_failed status={status}");
                    }
                    VTCompressionSessionInvalidate(session);
                    CFRelease(session as CFTypeRef);
                }
            }
            drain_pending(&self.ctx);
        }
    }

    impl Drop for Encoder {
        fn drop(&mut self) {
            // Documented order: CompleteFrames -> Invalidate -> CFRelease -> drain.
            self.drop_session();
        }
    }

    fn drain_pending(ctx: &CallbackCtx) {
        let mut queue = ctx
            .samples
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for sample in queue.drain(..) {
            unsafe { CFRelease(sample as CFTypeRef) };
        }
    }

    fn force_keyframe_properties() -> CFRetained<CFDictionary<CFString, CFBoolean>> {
        let key: &CFString = unsafe { &*kVTEncodeFrameOptionKey_ForceKeyFrame };
        CFDictionary::from_slices(&[key], &[CFBoolean::new(true)])
    }

    fn copy_to_pixel_buffer(
        image: &RgbaImage,
        width: u32,
        height: u32,
    ) -> Result<PixelBuffer, H264Error> {
        let mut raw: *mut CVPixelBuffer = ptr::null_mut();
        let status = unsafe {
            objc2_core_video::CVPixelBufferCreate(
                None,
                width as usize,
                height as usize,
                kCVPixelFormatType_32BGRA,
                None,
                NonNull::new_unchecked(&mut raw),
            )
        };
        if status != 0 || raw.is_null() {
            return Err(H264Error::Rejected(format!(
                "CVPixelBufferCreate(status={status})"
            )));
        }
        let buffer = PixelBuffer(raw);
        let pixel_buffer: &CVPixelBuffer = unsafe { &*raw };
        let lock = unsafe { objc2_core_video::CVPixelBufferLockBaseAddress(pixel_buffer, CVPixelBufferLockFlags(0)) };
        if lock != 0 {
            return Err(H264Error::Rejected(format!(
                "CVPixelBufferLockBaseAddress(status={lock})"
            )));
        }
        let base = objc2_core_video::CVPixelBufferGetBaseAddress(pixel_buffer);
        let stride = objc2_core_video::CVPixelBufferGetBytesPerRow(pixel_buffer);
        let copy_result = if base.is_null() || stride < width as usize * 4 {
            Err(H264Error::Rejected("invalid_pixel_buffer_layout".into()))
        } else {
            let length = stride
                .checked_mul(height as usize)
                .ok_or_else(|| H264Error::Rejected("invalid_pixel_buffer_layout".into()))?;
            let destination = unsafe { std::slice::from_raw_parts_mut(base as *mut u8, length) };
            super::copy_rgba_to_bgra(
                image.as_raw(),
                image.width() as usize * 4,
                destination,
                stride,
                width as usize,
                height as usize,
            )
        };
        let unlock = unsafe { objc2_core_video::CVPixelBufferUnlockBaseAddress(pixel_buffer, CVPixelBufferLockFlags(0)) };
        if unlock != 0 && copy_result.is_ok() {
            return Err(H264Error::Rejected(format!(
                "CVPixelBufferUnlockBaseAddress(status={unlock})"
            )));
        }
        copy_result?;
        Ok(buffer)
    }

    struct PixelBuffer(*mut CVPixelBuffer);

    impl PixelBuffer {
        fn as_ptr(&self) -> *mut CVPixelBuffer {
            self.0
        }
    }

    impl Drop for PixelBuffer {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { CFRelease(self.0 as CFTypeRef) };
            }
        }
    }

    unsafe fn extract(sample: *mut c_void) -> Result<Option<EncodedH264>, H264Error> {
        let sample: &CMSampleBuffer = unsafe { &*(sample as *const CMSampleBuffer) };
        // A sample without a format description or data buffer is not "no
        // output": it is a broken access unit. Return an error so the caller
        // forces a fresh IDR instead of silently dropping the frame.
        let format = unsafe { sample.format_description() }
            .ok_or_else(|| H264Error::Rejected("missing_format_description".into()))?;
        let block = unsafe { sample.data_buffer() }
            .ok_or_else(|| H264Error::Rejected("missing_data_buffer".into()))?;
        // One sample buffer carries one H.264 access unit. Multiple samples
        // would be separate access units and cannot be concatenated into one
        // AVCC sample.
        if unsafe { sample.num_samples() } > 1 {
            return Err(H264Error::Rejected("multiple_samples_in_buffer".into()));
        }
        // `CMSampleBufferGetTotalSampleSize` is 0 when the buffer has no sample
        // size array, so fall back to the whole block buffer. `CMBlockBufferGetDataLength`
        // covers every segment of a non-contiguous / multi-block buffer, and
        // `CMBlockBufferCopyDataBytes` walks those segments, unlike a raw pointer.
        let block_length = unsafe { block.data_length() };
        let total_sample_size = unsafe { sample.total_sample_size() };
        let length = if total_sample_size == 0 {
            block_length
        } else if total_sample_size <= block_length {
            total_sample_size
        } else {
            return Err(H264Error::Rejected("invalid_sample_length".into()));
        };
        if length == 0 || length > MAX_SAMPLE_BYTES {
            return Err(H264Error::Rejected("invalid_sample_length".into()));
        }
        let mut bytes = vec![0u8; length];
        let destination = NonNull::new(bytes.as_mut_ptr() as *mut c_void)
            .ok_or_else(|| H264Error::Rejected("empty_sample_buffer".into()))?;
        let status = unsafe { block.copy_data_bytes(0, length, destination) };
        if status != 0 {
            return Err(H264Error::Rejected(format!(
                "CMBlockBufferCopyDataBytes(status={status})"
            )));
        }
        let keyframe = unsafe { is_keyframe(sample) };
        let (sps, pps) = if keyframe {
            unsafe { parameter_sets(&format)? }
        } else {
            (Vec::new(), Vec::new())
        };
        Ok(Some(EncodedH264 {
            keyframe,
            width: 0, // filled by the caller from the session
            height: 0,
            sps,
            pps,
            sample: bytes,
        }))
    }

    unsafe fn is_keyframe(sample: &CMSampleBuffer) -> bool {
        let Some(attachments) = (unsafe { sample.sample_attachments_array(false) }) else {
            return true;
        };
        let array = CFRetained::as_ptr(&attachments).as_ptr() as CFTypeRef;
        if array.is_null() {
            return true;
        }
        let count = unsafe { CFArrayGetCount(array) };
        if count <= 0 {
            return true;
        }
        let dict = unsafe { CFArrayGetValueAtIndex(array, 0) };
        if dict.is_null() {
            return true;
        }
        // AVFoundation marks non-sync frames with `NotSync`; absent means IDR.
        let not_sync = unsafe { kCMSampleAttachmentKey_NotSync } as *const CFString as CFTypeRef;
        if not_sync.is_null() {
            return true;
        }
        (unsafe { CFDictionaryContainsKey(dict, not_sync) }) == 0
    }

    unsafe fn parameter_sets(
        format: &CMFormatDescription,
    ) -> Result<(Vec<u8>, Vec<u8>), H264Error> {
        // CMFormatDescription is an opaque CF type; the H.264 accessor accepts it.
        let mut sps = Vec::new();
        let mut pps = Vec::new();
        let mut count = 0usize;
        let mut header_length = 0i32;
        for index in 0..2usize {
            let mut pointer: *const u8 = ptr::null();
            let mut size = 0usize;
            let status = unsafe {
                objc2_core_media::CMVideoFormatDescriptionGetH264ParameterSetAtIndex(
                    format,
                    index,
                    &mut pointer,
                    &mut size,
                    &mut count,
                    &mut header_length,
                )
            };
            if status != 0 {
                return Err(H264Error::Rejected(format!(
                    "CMVideoFormatDescriptionGetH264ParameterSetAtIndex(index={index},status={status})"
                )));
            }
            if pointer.is_null() || size == 0 || size > 1024 {
                return Err(H264Error::Rejected("invalid_parameter_set".into()));
            }
            let bytes = unsafe { std::slice::from_raw_parts(pointer, size) }.to_vec();
            if index == 0 {
                sps = bytes;
            } else {
                pps = bytes;
            }
        }
        if count < 2 {
            return Err(H264Error::Rejected("missing_parameter_sets".into()));
        }
        if header_length != AVCC_HEADER_BYTES {
            return Err(H264Error::Rejected(format!(
                "unexpected_nal_length_size:{header_length}"
            )));
        }
        Ok((sps, pps))
    }

    /// Cheap session lifecycle check used by `available()` and tests.
    pub(super) fn probe() -> Result<(), String> {
        let encoder =
            Encoder::new(320, 240, 1_000, 30).map_err(|error| error.reason())?;
        drop(encoder);
        Ok(())
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};
    use std::time::Instant;

    /// Synthetic frames need no screen-recording permission (no TCC prompt).
    fn synthetic(width: u32, height: u32, tick: u32) -> RgbaImage {
        RgbaImage::from_fn(width, height, |x, y| {
            let a = ((x + tick) & 0xff) as u8;
            let b = ((y * 3 / 2 + tick * 5) & 0xff) as u8;
            Rgba([a, b, a.wrapping_add(b), 255])
        })
    }

    #[test]
    fn rgba_to_bgra_copy_honors_a_padded_destination_stride() {
        let (width, height) = (2usize, 3usize);
        let source_stride = width * 4;
        let destination_stride = width * 4 + 8; // padded, as CVPixelBuffer may return
        let source: Vec<u8> = (0..(source_stride * height) as u8).collect();
        let mut destination = vec![0xAAu8; destination_stride * height];
        copy_rgba_to_bgra(
            &source,
            source_stride,
            &mut destination,
            destination_stride,
            width,
            height,
        )
        .unwrap();
        for row in 0..height {
            for x in 0..width {
                let s = row * source_stride + x * 4;
                let d = row * destination_stride + x * 4;
                assert_eq!(
                    &destination[d..d + 4],
                    &[source[s + 2], source[s + 1], source[s], source[s + 3]],
                    "row={row} pixel={x}"
                );
            }
            // The row padding is never overwritten by the next row's copy.
            let pad = row * destination_stride + width * 4;
            assert!(
                destination[pad..pad + 8].iter().all(|byte| *byte == 0xAA),
                "row {row} padding must stay untouched"
            );
        }
        // A stride narrower than one row cannot be copied safely.
        let mut too_short = vec![0u8; destination_stride * height];
        assert!(copy_rgba_to_bgra(
            &source,
            source_stride,
            &mut too_short,
            width * 4 - 4,
            width,
            height
        )
        .is_err());
    }

    #[test]
    fn video_toolbox_encodes_idr_with_parameter_sets() {
        let _guard = lock_test_encoder();
        if !available() {
            eprintln!("skipping: VideoToolbox unavailable on this host");
            return;
        }
        let (width, height) = (1280, 720);
        let mut encoder = H264Encoder::new(width, height, 4_000, 30).expect("create encoder");
        assert_eq!(encoder.dimensions(), (1280, 720));

        let frames = 60u32;
        let mut keyframes = 0u32;
        let mut total_bytes = 0usize;
        let mut delta_bytes = 0usize;
        let mut deltas = 0u32;
        let started = Instant::now();
        for tick in 0..frames {
            let image = synthetic(width, height, tick * 4);
            let encoded = encoder
                .encode(&image, tick == 0)
                .expect("encode frame")
                .expect("video toolbox returned a frame");
            assert_even(&encoded);
            if encoded.keyframe {
                keyframes += 1;
                assert_eq!(encoded.sps.first().map(|b| b & 0x1f), Some(7));
                assert_eq!(encoded.pps.first().map(|b| b & 0x1f), Some(8));
                assert!(!encoded.sps.is_empty() && !encoded.pps.is_empty());
            } else {
                assert!(encoded.sps.is_empty() && encoded.pps.is_empty());
                deltas += 1;
                delta_bytes += encoded.sample.len();
            }
            // The AVCC sample must be structurally valid, exactly as framed.
            let mut offset = 0usize;
            while offset < encoded.sample.len() {
                assert!(encoded.sample.len() - offset >= 4, "truncated NAL length");
                let len = u32::from_be_bytes(
                    encoded.sample[offset..offset + 4].try_into().unwrap(),
                ) as usize;
                assert!(len > 0, "empty NAL");
                offset += 4 + len;
                assert!(offset <= encoded.sample.len(), "NAL length overflow");
            }
            assert_eq!(offset, encoded.sample.len(), "trailing sample bytes");
            total_bytes += encoded.sample.len() + encoded.sps.len() + encoded.pps.len();

            // Every packet must round-trip through the frozen wire framing.
            let frame = super::super::codec::H264Frame {
                header: super::super::codec::H264Header {
                    r#type: super::super::codec::H264FrameType::H264,
                    display: "1".into(),
                    seq: u64::from(tick),
                    w: encoded.width,
                    h: encoded.height,
                    key: encoded.keyframe,
                    ps: if encoded.keyframe {
                        vec![encoded.sps.len() as u32, encoded.pps.len() as u32]
                    } else {
                        Vec::new()
                    },
                    len: encoded.sample.len() as u32,
                },
                payload: encoded.payload(),
            };
            let packet = super::super::codec::encode_h264_frame(&frame).expect("frame valid");
            super::super::codec::decode_h264_frame(&packet).expect("frame round-trips");
        }
        let elapsed = started.elapsed();
        let fps = f64::from(frames) / elapsed.as_secs_f64().max(f64::MIN_POSITIVE);
        eprintln!(
            "h264 synthetic benchmark: frames={frames} size={width}x{height} keyframes={keyframes} \
             deltas={deltas} total_bytes={total_bytes} avg_delta_bytes={} fps={fps:.1} \
             avg_bytes_per_frame={:.0} bitrate_mbps={:.2}",
            delta_bytes / deltas.max(1) as usize,
            total_bytes as f64 / f64::from(frames),
            (total_bytes as f64 * 8.0) / elapsed.as_secs_f64() / 1_000_000.0,
        );
        assert!(keyframes >= 1, "at least the first frame must be an IDR");
        assert!(deltas > 0, "later frames must be deltas");
        assert!(total_bytes > 0);
        // Throughput is reported, not benchmarked: the whole lib test binary
        // runs in parallel, so a strict fps floor here would be flaky. The
        // floor only catches a completely stalled hardware encoder.
        assert!(
            fps >= 3.0,
            "host encoder should exceed 3fps at 720p, got {fps:.1}"
        );
    }

    fn assert_even(encoded: &EncodedH264) {
        assert_eq!(encoded.width % 2, 0);
        assert_eq!(encoded.height % 2, 0);
    }

    #[test]
    fn odd_capture_is_cropped_to_even_dimensions() {
        let _guard = lock_test_encoder();
        if !available() {
            eprintln!("skipping: VideoToolbox unavailable on this host");
            return;
        }
        let (width, height) = (1919, 1081);
        let mut encoder = H264Encoder::new(width, height, 4_000, 30).expect("create encoder");
        assert_eq!(encoder.dimensions(), (1918, 1080));
        let image = synthetic(width, height, 0);
        let encoded = encoder.encode(&image, true).unwrap().unwrap();
        assert_eq!((encoded.width, encoded.height), (1918, 1080));
        assert!(encoded.keyframe);
    }

    /// Forced keyframes must always carry SPS/PPS, including mid-stream.
    #[test]
    fn forced_keyframe_carries_parameter_sets() {
        let _guard = lock_test_encoder();
        if !available() {
            eprintln!("skipping: VideoToolbox unavailable on this host");
            return;
        }
        let mut encoder = H264Encoder::new(640, 360, 2_000, 30).expect("create encoder");
        let mut saw_delta = false;
        for tick in 0..20u32 {
            let encoded = encoder
                .encode(&synthetic(640, 360, tick), false)
                .unwrap()
                .unwrap();
            if !encoded.keyframe {
                saw_delta = true;
            }
        }
        assert!(saw_delta, "expected at least one delta frame");
        let forced = encoder
            .encode(&synthetic(640, 360, 99), true)
            .unwrap()
            .unwrap();
        assert!(forced.keyframe, "forced keyframe must be an IDR");
        assert!(!forced.sps.is_empty() && !forced.pps.is_empty());
    }
}
