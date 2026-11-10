//! Low-frame-rate desktop capture on a dedicated, joinable thread.
//!
//! `StreamEvent::Frame` is ready to send as a WebSocket binary message. Poll
//! `StreamReceiver::try_recv` from an async host, or use `recv_timeout` only on
//! a blocking thread. A receiver must preserve order; if the *transport* drops
//! a frame, it must discard subsequent deltas and call `request_keyframe()`.
//! The internal bounded queue repairs its own drops by replacing the backlog
//! with a full keyframe, never a delta whose baseline has been discarded.

use super::codec::{
    self, Frame, FrameHeader, FrameType, H264Frame, H264FrameType, H264Header, StreamCodec, Tile,
    TILE_SIZE,
};
use super::h264::{self, H264Encoder};
use super::{capture_raw, supported, ScreenError};
use image::{codecs::jpeg::JpegEncoder, Rgb, RgbImage, RgbaImage};
use std::{
    collections::VecDeque,
    sync::{
        mpsc::{RecvTimeoutError, TryRecvError},
        Arc, Condvar, Mutex, MutexGuard,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

/// JPEG defaults stay identical to the pre-P1 transport.
pub const DEFAULT_FPS: u32 = codec::DEFAULT_FPS_JPEG;
pub const MAX_FPS: u32 = codec::MAX_FPS_JPEG;

fn capture_error(message: impl ToString) -> ScreenError {
    ScreenError::CaptureFailed(message.to_string())
}

/// Exact raw-pixel differ. JPEG is lossy, but baselines are compared in raw
/// pixels so compression artifacts do not cause continuous idle updates.
pub struct FrameDiffer {
    previous: Option<RgbaImage>,
    display: Option<String>,
    quality: u8,
}

impl Default for FrameDiffer {
    fn default() -> Self {
        Self::new(75)
    }
}

impl FrameDiffer {
    pub fn new(quality: u8) -> Self {
        Self {
            previous: None,
            display: None,
            quality: quality.clamp(1, 100),
        }
    }

    pub fn reset(&mut self) {
        self.previous = None;
        self.display = None;
    }

    /// First image, display/size changes, explicit requests and all-tile
    /// changes produce one full JPEG. Identical images produce `None`.
    /// Callers dropping an output must force a keyframe before sending deltas.
    pub fn diff(
        &mut self,
        display: &str,
        seq: u64,
        image: &RgbaImage,
        force_keyframe: bool,
    ) -> Result<Option<Frame>, ScreenError> {
        let (w, h) = image.dimensions();
        if w == 0
            || h == 0
            || w > codec::MAX_DIMENSION
            || h > codec::MAX_DIMENSION
            || u64::from(w) * u64::from(h) > codec::MAX_PIXELS
            || display.trim().is_empty()
            || display.len() > 256
        {
            return Err(capture_error("invalid capture dimensions or display id"));
        }
        let key = force_keyframe
            || self.display.as_deref() != Some(display)
            || self
                .previous
                .as_ref()
                .is_none_or(|old| old.dimensions() != (w, h));
        let mut changed = Vec::new();
        if !key {
            let old = self.previous.as_ref().unwrap();
            for y in (0..h).step_by(TILE_SIZE as usize) {
                for x in (0..w).step_by(TILE_SIZE as usize) {
                    let tw = TILE_SIZE.min(w - x);
                    let th = TILE_SIZE.min(h - y);
                    if !tile_equal(old, image, x, y, tw, th) {
                        changed.push((x, y, tw, th));
                    }
                }
            }
            if changed.is_empty() {
                return Ok(None);
            }
        }
        let total_tiles = w.div_ceil(TILE_SIZE) as usize * h.div_ceil(TILE_SIZE) as usize;
        let key = key || changed.len() == total_tiles || changed.len() > codec::MAX_TILES;
        if key {
            changed = vec![(0, 0, w, h)];
        }
        let mut jpeg = Vec::new();
        let mut tiles = Vec::with_capacity(changed.len());
        for (x, y, w, h) in changed {
            let start = jpeg.len();
            let rgb = RgbImage::from_fn(w, h, |dx, dy| {
                let pixel = image.get_pixel(x + dx, y + dy);
                Rgb([pixel[0], pixel[1], pixel[2]])
            });
            JpegEncoder::new_with_quality(&mut jpeg, self.quality)
                .encode_image(&rgb)
                .map_err(capture_error)?;
            tiles.push(Tile {
                x,
                y,
                w,
                h,
                len: (jpeg.len() - start) as u32,
            });
            if jpeg.len() > codec::MAX_PACKET_BYTES {
                return Err(capture_error("encoded capture exceeds packet limit"));
            }
        }
        self.previous = Some(image.clone());
        self.display = Some(display.to_owned());
        Ok(Some(Frame {
            header: FrameHeader {
                r#type: if key {
                    FrameType::Key
                } else {
                    FrameType::Tiles
                },
                display: display.to_owned(),
                seq,
                w,
                h,
                tiles,
            },
            jpeg,
        }))
    }
}

fn tile_equal(a: &RgbaImage, b: &RgbaImage, x: u32, y: u32, w: u32, h: u32) -> bool {
    let stride = a.width() as usize * 4;
    let width = w as usize * 4;
    (y..y + h).all(|row| {
        let start = row as usize * stride + x as usize * 4;
        a.as_raw()[start..start + width] == b.as_raw()[start..start + width]
    })
}

#[derive(Debug, Clone)]
pub struct StreamConfig {
    pub display: String,
    /// Clamped per negotiated codec: 1..=5 for JPEG, 1..=30 for H.264.
    /// A value of zero selects the codec default (3fps / 30fps).
    pub fps: u32,
    /// Capture width, clamped to 1..=1920; zero selects 1920.
    pub max_width: u32,
    pub quality: u8,
    /// Clamped to 1..=4. No unbounded frame backlog is allowed.
    pub queue_capacity: usize,
    /// Requested transport codec; falls back to JPEG when H.264 is unavailable.
    pub codec: StreamCodec,
    /// H.264 average bitrate in kbps, clamped to 500..=20000.
    pub bitrate_kbps: u32,
}

impl Default for StreamConfig {
    fn default() -> Self {
        Self {
            display: String::new(),
            fps: DEFAULT_FPS,
            max_width: 1920,
            quality: 75,
            queue_capacity: 2,
            codec: StreamCodec::Jpeg,
            bitrate_kbps: codec::DEFAULT_BITRATE_KBPS,
        }
    }
}

#[derive(Debug)]
pub enum StreamEvent {
    Frame(Vec<u8>),
    /// Terminal error; the receiver disconnects after this event is consumed.
    Error(ScreenError),
}

struct State {
    queue: VecDeque<StreamEvent>,
    display: String,
    generation: u64,
    force_keyframe: bool,
    stopped: bool,
    closed: bool,
    receiver_alive: bool,
}

struct Shared {
    state: Mutex<State>,
    changed: Condvar,
    capacity: usize,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn fail(&self, error: ScreenError) {
        let mut state = self.lock();
        if !state.stopped && state.receiver_alive {
            state.queue.clear();
            state.queue.push_back(StreamEvent::Error(error));
        }
        state.closed = true;
        self.changed.notify_all();
    }
}

/// One consumer, with standard mpsc-style empty/disconnected distinctions.
/// Dropping the receiver also requests capture-thread shutdown.
pub struct StreamReceiver {
    shared: Arc<Shared>,
}

impl StreamReceiver {
    pub fn try_recv(&self) -> Result<StreamEvent, TryRecvError> {
        let mut state = self.shared.lock();
        if let Some(event) = state.queue.pop_front() {
            Ok(event)
        } else if state.closed || state.stopped {
            Err(TryRecvError::Disconnected)
        } else {
            Err(TryRecvError::Empty)
        }
    }

    pub fn recv_timeout(&self, timeout: Duration) -> Result<StreamEvent, RecvTimeoutError> {
        let start = Instant::now();
        let mut state = self.shared.lock();
        loop {
            if let Some(event) = state.queue.pop_front() {
                return Ok(event);
            }
            if state.closed || state.stopped {
                return Err(RecvTimeoutError::Disconnected);
            }
            let remaining = timeout.saturating_sub(start.elapsed());
            if remaining.is_zero() {
                return Err(RecvTimeoutError::Timeout);
            }
            state = self
                .shared
                .changed
                .wait_timeout(state, remaining)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }
}

impl Drop for StreamReceiver {
    fn drop(&mut self) {
        let mut state = self.shared.lock();
        state.receiver_alive = false;
        state.stopped = true;
        state.queue.clear();
        self.shared.changed.notify_all();
    }
}

/// Owns the dedicated capture thread; dropping it always signals and joins.
/// An OS capture already in progress must return before the join can finish.
pub struct StreamSession {
    shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
    /// Codec negotiated at start; reported by `meta` so clients never guess.
    codec: StreamCodec,
}

impl StreamSession {
    pub fn start(config: StreamConfig) -> Result<(Self, StreamReceiver), ScreenError> {
        if !supported() {
            return Err(ScreenError::Unsupported);
        }
        Self::start_with_capture(config, capture_raw)
    }

    fn start_with_capture<F>(
        mut config: StreamConfig,
        capture: F,
    ) -> Result<(Self, StreamReceiver), ScreenError>
    where
        F: FnMut(&str, u32) -> Result<RgbaImage, ScreenError> + Send + 'static,
    {
        if config.display.trim().is_empty() || config.display.len() > 256 {
            return Err(ScreenError::DisplayNotFound);
        }
        // H.264 is only advertised when a VideoToolbox session can be created;
        // a request for it on Windows/Linux or a broken host falls back to JPEG.
        let requested_codec = config.codec;
        if requested_codec == StreamCodec::H264 && !h264::available() {
            config.codec = StreamCodec::Jpeg;
            log::warn!("desktop_h264_fallback requested=h264 codec=jpeg reason=unavailable");
        }
        config.fps = codec::clamp_fps(config.codec, Some(config.fps));
        config.bitrate_kbps = codec::clamp_bitrate_kbps(Some(config.bitrate_kbps));
        config.max_width = if config.max_width == 0 {
            1920
        } else {
            config.max_width.min(1920)
        };
        config.quality = config.quality.clamp(1, 100);
        let effective_codec = config.codec;
        if requested_codec != effective_codec {
            log::info!(
                "desktop_codec_negotiated requested={} codec={}",
                requested_codec.as_str(),
                effective_codec.as_str()
            );
        }
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                queue: VecDeque::new(),
                display: config.display.clone(),
                generation: 0,
                force_keyframe: true,
                stopped: false,
                closed: false,
                receiver_alive: true,
            }),
            changed: Condvar::new(),
            capacity: config.queue_capacity.clamp(1, 4),
        });
        let thread_shared = shared.clone();
        let worker = thread::Builder::new()
            .name("desktop-capture".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    capture_loop(&thread_shared, &config, capture);
                }));
                if result.is_err() {
                    thread_shared.fail(capture_error("desktop capture worker panicked"));
                }
                thread_shared.lock().closed = true;
                thread_shared.changed.notify_all();
            })
            .map_err(capture_error)?;
        Ok((
            Self {
                shared: shared.clone(),
                worker: Some(worker),
                codec: effective_codec,
            },
            StreamReceiver { shared },
        ))
    }

    /// Codec actually negotiated for this session (JPEG after any fallback).
    pub fn codec(&self) -> StreamCodec {
        self.codec
    }

    pub fn request_keyframe(&self) {
        let mut state = self.shared.lock();
        state.force_keyframe = true;
        state.generation = state.generation.wrapping_add(1);
        state.queue.clear();
        self.shared.changed.notify_all();
    }

    /// Clears queued frames for the old display. An in-flight old capture is
    /// discarded by generation, and the new display starts with a keyframe.
    pub fn switch_display(&self, display: impl Into<String>) -> Result<(), ScreenError> {
        let display = display.into();
        if display.trim().is_empty() || display.len() > 256 {
            return Err(ScreenError::DisplayNotFound);
        }
        let mut state = self.shared.lock();
        if state.stopped || state.closed {
            return Err(capture_error("stream already stopped"));
        }
        state.display = display;
        state.generation = state.generation.wrapping_add(1);
        state.force_keyframe = true;
        state.queue.clear();
        self.shared.changed.notify_all();
        Ok(())
    }

    pub fn stop(&mut self) {
        {
            let mut state = self.shared.lock();
            state.stopped = true;
            state.queue.clear();
            self.shared.changed.notify_all();
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for StreamSession {
    fn drop(&mut self) {
        self.stop();
    }
}

fn capture_loop<F>(shared: &Shared, config: &StreamConfig, mut capture: F)
where
    F: FnMut(&str, u32) -> Result<RgbaImage, ScreenError>,
{
    let interval = Duration::from_secs_f64(1.0 / f64::from(config.fps));
    let mut next_capture = Instant::now();
    let mut differ = FrameDiffer::new(config.quality);
    let mut h264_state = H264State::default();
    let mut seq = 0u64;
    loop {
        let (display, generation, force_keyframe) = {
            let mut state = shared.lock();
            loop {
                if state.stopped || !state.receiver_alive {
                    return;
                }
                let remaining = next_capture.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    break;
                }
                state = shared
                    .changed
                    .wait_timeout(state, remaining)
                    .unwrap_or_else(|e| e.into_inner())
                    .0;
            }
            (
                state.display.clone(),
                state.generation,
                std::mem::take(&mut state.force_keyframe),
            )
        };
        // Count from capture start: even a flood of switch/keyframe requests
        // cannot exceed the configured maximum capture rate.
        next_capture = Instant::now() + interval;
        let capture_started = Instant::now();
        let result: Result<(), ScreenError> = (|| {
            let image = capture(&display, config.max_width)?;
            let published = match config.codec {
                StreamCodec::H264 => capture_h264(
                    shared,
                    generation,
                    &display,
                    seq,
                    &image,
                    force_keyframe,
                    &mut h264_state,
                    config.bitrate_kbps,
                    config.fps,
                )?,
                StreamCodec::Jpeg => {
                    match differ.diff(&display, seq, &image, force_keyframe)? {
                        Some(frame) => {
                            publish_frame(shared, generation, frame, &image, &mut differ)?;
                            true
                        }
                        None => false,
                    }
                }
            };
            if published {
                seq = seq
                    .checked_add(1)
                    .ok_or_else(|| capture_error("frame sequence exhausted"))?;
            }
            Ok(())
        })();
        log::info!(
            "operation=desktop_capture stable_code={} duration_ms={}",
            result.as_ref().err().map_or("OK", ScreenError::code),
            capture_started.elapsed().as_millis()
        );
        if let Err(error) = result {
            let mut state = shared.lock();
            if state.stopped {
                return;
            }
            if state.generation != generation {
                // A display switch supersedes failures of an old capture.
                differ.reset();
                continue;
            }
            state.queue.clear();
            state.queue.push_back(StreamEvent::Error(error));
            state.closed = true;
            shared.changed.notify_all();
            return;
        }
    }
}

/// Single producer only. Locks cover tiny queue/control operations, never OS
/// capture, JPEG encoding or socket I/O. On overflow discard the entire stale
/// dependency chain, not merely its oldest keyframe, and publish a new key.
fn publish_frame(
    shared: &Shared,
    generation: u64,
    mut frame: Frame,
    image: &RgbaImage,
    differ: &mut FrameDiffer,
) -> Result<(), ScreenError> {
    let dropped = {
        let mut state = shared.lock();
        if state.stopped || !state.receiver_alive || state.generation != generation {
            return Ok(());
        }
        if state.queue.len() >= shared.capacity {
            state.queue.clear();
            true
        } else {
            false
        }
    };
    if dropped && frame.header.r#type != FrameType::Key {
        frame = differ
            .diff(&frame.header.display, frame.header.seq, image, true)?
            .ok_or_else(|| capture_error("keyframe was not produced"))?;
    }
    let packet = codec::encode_frame(&frame).map_err(capture_error)?;
    let mut state = shared.lock();
    if !state.stopped && state.receiver_alive && state.generation == generation {
        state.queue.push_back(StreamEvent::Frame(packet));
        shared.changed.notify_all();
    }
    Ok(())
}

#[derive(Default)]
struct H264State {
    encoder: Option<H264Encoder>,
    /// Last encoded canvas: identical frames are skipped entirely.
    last_frame: Option<(u32, u32, Vec<u8>)>,
    /// Set after a dropped frame or queue overflow: the next frame must be an IDR.
    force_keyframe: bool,
}

#[derive(Debug, PartialEq, Eq)]
enum H264Publish {
    Sent,
    /// The queue overflowed and this delta lost its baseline: wait for an IDR.
    DroppedNeedsKeyframe,
    /// A display switch or stop superseded this frame.
    Stale,
}

/// Single producer only, like `publish_frame`. On overflow the whole backlog is
/// discarded; a delta frame is never published after its baseline was dropped.
fn publish_h264_packet(
    shared: &Shared,
    generation: u64,
    keyframe: bool,
    packet: Vec<u8>,
) -> Result<H264Publish, ScreenError> {
    let mut state = shared.lock();
    if state.stopped || !state.receiver_alive || state.generation != generation {
        return Ok(H264Publish::Stale);
    }
    let overflow = state.queue.len() >= shared.capacity;
    if overflow {
        state.queue.clear();
        if !keyframe {
            return Ok(H264Publish::DroppedNeedsKeyframe);
        }
    }
    state.queue.push_back(StreamEvent::Frame(packet));
    shared.changed.notify_all();
    Ok(H264Publish::Sent)
}

/// One H.264 tick: skip unchanged frames, encode, then publish under the same
/// bounded-queue and keyframe rules as the JPEG path.
#[allow(clippy::too_many_arguments)]
fn capture_h264(
    shared: &Shared,
    generation: u64,
    display: &str,
    seq: u64,
    image: &RgbaImage,
    requested_keyframe: bool,
    state: &mut H264State,
    bitrate_kbps: u32,
    fps: u32,
) -> Result<bool, ScreenError> {
    let (width, height) = image.dimensions();
    if width == 0
        || height == 0
        || width > codec::MAX_DIMENSION
        || height > codec::MAX_DIMENSION
        || u64::from(width) * u64::from(height) > codec::MAX_PIXELS
        || display.trim().is_empty()
        || display.len() > 256
    {
        return Err(capture_error("invalid capture dimensions or display id"));
    }
    let (even_width, even_height) = (h264::even_dimension(width), h264::even_dimension(height));
    if !h264::can_encode(even_width, even_height) {
        return Err(capture_error("capture too small for h264"));
    }
    let mut force_keyframe = requested_keyframe || state.force_keyframe;
    // Unchanged screen: the client already holds a decodable frame.
    if !force_keyframe
        && state.last_frame.as_ref().is_some_and(|(w, h, bytes)| {
            *w == width && *h == height && bytes.as_slice() == image.as_raw()
        })
    {
        return Ok(false);
    }
    // A new session starts with an IDR, which also covers display switches and
    // resolution changes (both recreate the encoder).
    if state
        .encoder
        .as_ref()
        .map(H264Encoder::dimensions)
        != Some((even_width, even_height))
    {
        state.encoder = None;
        force_keyframe = true;
        match H264Encoder::new(even_width, even_height, bitrate_kbps, fps) {
            Ok(encoder) => {
                log::info!(
                    "desktop_h264_encoder_ready width={even_width} height={even_height} \
                     bitrate_kbps={bitrate_kbps} fps={fps}"
                );
                state.encoder = Some(encoder);
            }
            Err(error) => {
                log::warn!("desktop_h264_encoder_failed reason={}", error.reason());
                return Err(capture_error("h264 encoder unavailable"));
            }
        }
    }
    let encode_result = match state.encoder.as_mut() {
        Some(encoder) => encoder.encode(image, force_keyframe),
        None => return Err(capture_error("h264 encoder unavailable")),
    };
    let encoded = match encode_result {
        Ok(Some(encoded)) => encoded,
        Ok(None) => return Ok(false),
        Err(error) => {
            log::warn!("desktop_h264_encode_failed reason={}", error.reason());
            state.encoder = None;
            state.force_keyframe = true;
            return Err(capture_error("h264 encoding failed"));
        }
    };
    let header = H264Header {
        r#type: H264FrameType::H264,
        display: display.to_owned(),
        seq,
        w: encoded.width,
        h: encoded.height,
        key: encoded.keyframe,
        ps: if encoded.keyframe {
            vec![encoded.sps.len() as u32, encoded.pps.len() as u32]
        } else {
            Vec::new()
        },
        len: encoded.sample.len() as u32,
    };
    let packet = codec::encode_h264_frame(&H264Frame {
        header,
        payload: encoded.payload(),
    })
    .map_err(capture_error)?;
    let outcome = publish_h264_packet(shared, generation, encoded.keyframe, packet)?;
    state.last_frame = Some((width, height, image.as_raw().clone()));
    match outcome {
        H264Publish::Sent => {
            state.force_keyframe = false;
            Ok(true)
        }
        H264Publish::DroppedNeedsKeyframe => {
            state.force_keyframe = true;
            Ok(false)
        }
        H264Publish::Stale => Ok(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

    fn canvas() -> RgbaImage {
        RgbaImage::from_pixel(256, 256, Rgba([0, 0, 0, 255]))
    }

    fn paint(image: &mut RgbaImage, x: u32, y: u32, color: [u8; 4]) {
        for dy in 0..128 {
            for dx in 0..128 {
                image.put_pixel(x + dx, y + dy, Rgba(color));
            }
        }
    }

    fn event_frame(event: StreamEvent) -> Frame {
        match event {
            StreamEvent::Frame(packet) => codec::decode_frame(&packet).unwrap(),
            StreamEvent::Error(error) => panic!("unexpected stream error: {error:?}"),
        }
    }

    fn shared(capacity: usize) -> Shared {
        Shared {
            state: Mutex::new(State {
                queue: VecDeque::new(),
                display: "1".into(),
                generation: 0,
                force_keyframe: false,
                stopped: false,
                closed: false,
                receiver_alive: true,
            }),
            changed: Condvar::new(),
            capacity,
        }
    }

    #[test]
    fn no_change_produces_no_delta() {
        let mut differ = FrameDiffer::default();
        let image = canvas();
        let first = differ.diff("1", 1, &image, false).unwrap().unwrap();
        assert_eq!(first.header.r#type, FrameType::Key);
        assert_eq!(first.header.tiles.len(), 1);
        assert!(differ.diff("1", 2, &image, false).unwrap().is_none());
    }

    #[test]
    fn one_pixel_change_sends_only_one_tile() {
        let mut differ = FrameDiffer::default();
        let mut image = canvas();
        differ.diff("1", 1, &image, false).unwrap();
        image.put_pixel(200, 100, Rgba([255, 0, 0, 255]));
        let frame = differ.diff("1", 2, &image, false).unwrap().unwrap();
        assert_eq!(frame.header.r#type, FrameType::Tiles);
        assert_eq!(frame.header.tiles.len(), 1);
        let tile = &frame.header.tiles[0];
        assert_eq!((tile.x, tile.y, tile.w, tile.h), (128, 0, 128, 128));
        assert_eq!(
            codec::decode_frame(&codec::encode_frame(&frame).unwrap()).unwrap(),
            frame
        );
    }

    #[test]
    fn full_change_resize_switch_and_request_send_keyframes() {
        let mut differ = FrameDiffer::default();
        differ.diff("1", 1, &canvas(), false).unwrap();
        let image = RgbaImage::from_pixel(256, 256, Rgba([255, 255, 255, 255]));
        let full = differ.diff("1", 2, &image, false).unwrap().unwrap();
        assert_eq!(full.header.r#type, FrameType::Key);
        assert_eq!(full.header.tiles.len(), 1);
        for (display, force) in [("1", true), ("2", false)] {
            assert_eq!(
                differ
                    .diff(display, 3, &image, force)
                    .unwrap()
                    .unwrap()
                    .header
                    .r#type,
                FrameType::Key
            );
        }
        let resized = RgbaImage::from_pixel(129, 17, Rgba([0, 0, 0, 255]));
        assert_eq!(
            differ
                .diff("2", 4, &resized, false)
                .unwrap()
                .unwrap()
                .header
                .r#type,
            FrameType::Key
        );
    }

    #[test]
    fn edge_tiles_have_actual_remaining_dimensions() {
        let mut differ = FrameDiffer::default();
        let mut image = RgbaImage::from_pixel(129, 131, Rgba([0, 0, 0, 255]));
        differ.diff("1", 0, &image, false).unwrap();
        image.put_pixel(128, 130, Rgba([255, 0, 0, 255]));
        let frame = differ.diff("1", 1, &image, false).unwrap().unwrap();
        let t = &frame.header.tiles[0];
        assert_eq!((t.x, t.y, t.w, t.h), (128, 128, 1, 3));
        codec::encode_frame(&frame).unwrap();
    }

    #[test]
    fn overflow_discards_dependency_chain_and_reconstructs_latest_canvas() {
        for capacity in [1, 2] {
            let shared = shared(capacity);
            let mut differ = FrameDiffer::default();
            let mut image = canvas();
            let first = differ.diff("1", 0, &image, false).unwrap().unwrap();
            publish_frame(&shared, 0, first, &image, &mut differ).unwrap();
            paint(&mut image, 0, 0, [255, 0, 0, 255]);
            let delta = differ.diff("1", 1, &image, false).unwrap().unwrap();
            assert_eq!(delta.header.r#type, FrameType::Tiles);
            publish_frame(&shared, 0, delta, &image, &mut differ).unwrap();
            paint(&mut image, 128, 128, [0, 0, 255, 255]);
            let delta = differ.diff("1", 2, &image, false).unwrap().unwrap();
            publish_frame(&shared, 0, delta, &image, &mut differ).unwrap();
            let mut state = shared.lock();
            assert_eq!(state.queue.len(), 1);
            let recovered = event_frame(state.queue.pop_front().unwrap());
            assert_eq!(recovered.header.r#type, FrameType::Key);
            assert_eq!(recovered.header.seq, 2);
            let decoded = image::load_from_memory(&recovered.jpeg).unwrap().to_rgb8();
            assert!(decoded.get_pixel(64, 64)[0] > 240);
            assert!(decoded.get_pixel(192, 192)[2] > 240);
        }
    }

    #[test]
    fn stale_inflight_display_frame_is_discarded() {
        let shared = shared(1);
        shared.lock().generation = 1;
        let mut differ = FrameDiffer::default();
        let image = canvas();
        let frame = differ.diff("1", 0, &image, false).unwrap().unwrap();
        publish_frame(&shared, 0, frame, &image, &mut differ).unwrap();
        assert!(shared.lock().queue.is_empty());
    }

    #[test]
    fn h264_overflow_drops_deltas_until_an_idr() {
        let stream = shared(1);
        assert_eq!(
            publish_h264_packet(&stream, 0, true, vec![1]).unwrap(),
            H264Publish::Sent
        );
        // Queue full: the delta's baseline would be gone, so it is dropped and
        // the backlog cleared instead of forwarding later deltas.
        assert_eq!(
            publish_h264_packet(&stream, 0, false, vec![2]).unwrap(),
            H264Publish::DroppedNeedsKeyframe
        );
        assert!(stream.lock().queue.is_empty());
        // An IDR is self-contained and always recoverable, even into a full queue.
        assert_eq!(
            publish_h264_packet(&stream, 0, true, vec![3]).unwrap(),
            H264Publish::Sent
        );
        assert_eq!(
            publish_h264_packet(&stream, 0, true, vec![4]).unwrap(),
            H264Publish::Sent
        );
        // Frames from a superseded display generation are never published.
        assert_eq!(
            publish_h264_packet(&stream, 7, true, vec![5]).unwrap(),
            H264Publish::Stale
        );
    }

    #[test]
    fn h264_unchanged_frames_and_invalid_geometry_are_skipped() {
        let stream = shared(2);
        let mut state = H264State::default();
        let image = canvas();
        // A tiny canvas has no even dimensions to encode at.
        assert!(capture_h264(
            &stream, 0, "1", 0, &RgbaImage::from_pixel(1, 1, Rgba([0, 0, 0, 255])), true,
            &mut state, 4000, 30
        )
        .is_err());
        // Pretend a frame was already encoded and is identical: no packet.
        state.last_frame = Some((256, 256, image.as_raw().clone()));
        assert!(!capture_h264(&stream, 0, "1", 0, &image, false, &mut state, 4000, 30).unwrap());
        assert!(stream.lock().queue.is_empty());
        // Odd geometry is cropped, and h264::even_dimension is the single rule.
        assert_eq!(h264::even_dimension(1919), 1918);
        assert_eq!(h264::even_dimension(1080), 1080);
    }

    /// Full H.264 session over synthetic frames: negotiation, IDR first, then
    /// deltas, all through the frozen wire framing. Needs no screen permission.
    #[test]
    fn h264_session_emits_idr_then_deltas() {
        if !h264::available() {
            eprintln!("skipping: VideoToolbox unavailable on this host");
            return;
        }
        let tick = Arc::new(AtomicU64::new(0));
        let counter = tick.clone();
        let config = StreamConfig {
            display: "1".into(),
            fps: 30,
            max_width: 256,
            quality: 80,
            queue_capacity: 4,
            codec: StreamCodec::H264,
            bitrate_kbps: 4_000,
        };
        let (mut session, receiver) = StreamSession::start_with_capture(config, move |_, _| {
            let n = counter.fetch_add(1, Ordering::SeqCst) as u32;
            Ok(RgbaImage::from_fn(256, 256, |x, y| {
                Rgba([
                    (x as u8).wrapping_add(n as u8),
                    y as u8,
                    (n & 0xff) as u8,
                    255,
                ])
            }))
        })
        .unwrap();
        assert_eq!(session.codec(), StreamCodec::H264);
        let first = match receiver.recv_timeout(Duration::from_secs(5)).unwrap() {
            StreamEvent::Frame(packet) => codec::decode_h264_frame(&packet).unwrap(),
            StreamEvent::Error(error) => panic!("unexpected stream error: {error:?}"),
        };
        assert!(first.header.key, "the first frame must be an IDR");
        assert_eq!(first.header.ps.len(), 2);
        assert_eq!(first.sps()[0] & 0x1f, 7);
        assert_eq!(first.pps()[0] & 0x1f, 8);
        assert_eq!((first.header.w, first.header.h), (256, 256));
        let mut deltas = 0;
        for _ in 0..6 {
            let frame = match receiver.recv_timeout(Duration::from_secs(5)) {
                Ok(StreamEvent::Frame(packet)) => codec::decode_h264_frame(&packet).unwrap(),
                Ok(StreamEvent::Error(error)) => panic!("unexpected stream error: {error:?}"),
                Err(error) => panic!("missing frame: {error:?}"),
            };
            if !frame.header.key {
                deltas += 1;
                assert!(frame.header.ps.is_empty());
            }
        }
        assert!(deltas > 0, "later frames must be deltas");
        session.stop();
    }

    #[test]
    fn thread_keyframe_switch_rate_limit_and_stop() {
        let starts = Arc::new(Mutex::new(Vec::new()));
        let capture_starts = starts.clone();
        let config = StreamConfig {
            display: "1".into(),
            fps: 99,
            max_width: 9999,
            ..StreamConfig::default()
        };
        let (mut session, receiver) = StreamSession::start_with_capture(config, move |_, width| {
            assert_eq!(width, 1920);
            capture_starts.lock().unwrap().push(Instant::now());
            Ok(canvas())
        })
        .unwrap();
        let first = event_frame(receiver.recv_timeout(Duration::from_secs(2)).unwrap());
        assert_eq!(first.header.r#type, FrameType::Key);
        assert!(matches!(receiver.try_recv(), Err(TryRecvError::Empty)));
        session.request_keyframe();
        let second = event_frame(receiver.recv_timeout(Duration::from_secs(2)).unwrap());
        assert_eq!(second.header.r#type, FrameType::Key);
        session.switch_display("2").unwrap();
        let switched = event_frame(receiver.recv_timeout(Duration::from_secs(2)).unwrap());
        assert_eq!(switched.header.display, "2");
        assert_eq!(switched.header.r#type, FrameType::Key);
        let starts = starts.lock().unwrap();
        for pair in starts.windows(2) {
            assert!(pair[1].duration_since(pair[0]) >= Duration::from_millis(190));
        }
        drop(starts);
        session.stop();
        assert!(matches!(
            receiver.try_recv(),
            Err(TryRecvError::Disconnected)
        ));
        assert!(session.worker.is_none());
    }

    #[test]
    fn idle_receiver_times_out_and_capture_errors_disconnect() {
        let (session, receiver) = StreamSession::start_with_capture(
            StreamConfig {
                display: "1".into(),
                ..StreamConfig::default()
            },
            |_, _| Ok(canvas()),
        )
        .unwrap();
        receiver.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(matches!(
            receiver.recv_timeout(Duration::from_millis(30)),
            Err(RecvTimeoutError::Timeout)
        ));
        drop(session);
        assert!(matches!(
            receiver.try_recv(),
            Err(TryRecvError::Disconnected)
        ));

        let (_session, receiver) = StreamSession::start_with_capture(
            StreamConfig {
                display: "1".into(),
                ..StreamConfig::default()
            },
            |_, _| Err(ScreenError::ScreenPermissionRequired),
        )
        .unwrap();
        assert!(matches!(
            receiver.recv_timeout(Duration::from_secs(2)).unwrap(),
            StreamEvent::Error(ScreenError::ScreenPermissionRequired)
        ));
        assert!(matches!(
            receiver.try_recv(),
            Err(TryRecvError::Disconnected)
        ));
    }

    #[test]
    fn drop_joins_worker_and_receiver_drop_stops_capture() {
        struct Dropped(Arc<AtomicBool>);
        impl Drop for Dropped {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }
        let dropped = Arc::new(AtomicBool::new(false));
        let guard = Dropped(dropped.clone());
        let (session, receiver) = StreamSession::start_with_capture(
            StreamConfig {
                display: "1".into(),
                ..StreamConfig::default()
            },
            move |_, _| {
                let _ = &guard;
                Ok(canvas())
            },
        )
        .unwrap();
        receiver.recv_timeout(Duration::from_secs(2)).unwrap();
        drop(receiver);
        drop(session);
        assert!(dropped.load(Ordering::SeqCst));
    }

    #[test]
    fn worker_panics_are_reported_as_terminal_errors() {
        let (_session, receiver) = StreamSession::start_with_capture(
            StreamConfig {
                display: "1".into(),
                ..StreamConfig::default()
            },
            |_, _| panic!("mock capture panic"),
        )
        .unwrap();
        assert!(matches!(
            receiver.recv_timeout(Duration::from_secs(2)).unwrap(),
            StreamEvent::Error(ScreenError::CaptureFailed(_))
        ));
        assert!(matches!(
            receiver.try_recv(),
            Err(TryRecvError::Disconnected)
        ));
    }
}
