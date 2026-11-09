//! Bounded, generation-scoped desktop input. The host must explicitly `rearm`
//! after consent, then create a fresh controller. Success means queue admission,
//! not delivery; asynchronous failures revoke the entire input generation.
//! No input (including Enigo's automatic key cleanup) is emitted after disarm.

use super::{list_displays, supported, DisplayInfo, ScreenError};
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, OnceLock, Weak};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const QUEUE_CAPACITY: usize = 64;
const MAX_TEXT_BYTES: usize = 4096;
const MAX_NATIVE_EVENTS: usize = 120;
const RATE_WINDOW: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Modifiers {
    #[serde(alias = "control")]
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    #[serde(alias = "command")]
    pub meta: bool,
}

#[derive(Default)]
struct Limiter {
    events: VecDeque<Instant>,
}

impl Limiter {
    fn delay(&mut self, now: Instant, cost: usize) -> Duration {
        while self
            .events
            .front()
            .is_some_and(|at| now.duration_since(*at) >= RATE_WINDOW)
        {
            self.events.pop_front();
        }
        if self.events.len() + cost <= MAX_NATIVE_EVENTS {
            Duration::ZERO
        } else {
            RATE_WINDOW.saturating_sub(
                now.duration_since(self.events[self.events.len() + cost - MAX_NATIVE_EVENTS - 1]),
            )
        }
    }

    fn record(&mut self, now: Instant, cost: usize) {
        self.events.extend(std::iter::repeat_n(now, cost));
    }
}

#[derive(Default)]
struct Gate {
    armed: bool,
    generation: u64,
    limiter: Limiter,
    queues: Vec<Weak<Queue>>,
}

impl Gate {
    fn check(&self, generation: u64) -> Result<(), ScreenError> {
        if self.armed && self.generation == generation {
            Ok(())
        } else {
            Err(failed(
                "desktop input is disarmed; rearm and create a new controller",
            ))
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
        self.generation = self.generation.wrapping_add(1);
        for queue in self.queues.drain(..).filter_map(|q| q.upgrade()) {
            queue.stop();
        }
        // Do not reset the limiter: kill/rearm cannot bypass the global budget.
    }
}

fn global() -> &'static Mutex<Gate> {
    static GATE: OnceLock<Mutex<Gate>> = OnceLock::new();
    GATE.get_or_init(|| Mutex::new(Gate::default()))
}

fn revoke_generation(generation: u64, queue: &Queue) {
    let mut gate = global().lock().unwrap_or_else(|e| e.into_inner());
    let stopped = queue
        .pending
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .stopped;
    if gate.generation == generation && !stopped {
        gate.disarm();
    }
}

fn failed(reason: &str) -> ScreenError {
    ScreenError::InputFailed(reason.to_owned())
}

fn audit(operation: &'static str, start: Instant, result: &Result<(), ScreenError>) {
    let code = result.as_ref().err().map_or("OK", ScreenError::code);
    log::info!(
        "operation={operation} stable_code={code} duration_ms={}",
        start.elapsed().as_millis()
    );
}

/// Synchronous revocation barrier. Once this returns, no old-generation native
/// call can start. Queued work is discarded and waiting workers are awakened.
/// Cleanup deliberately does not release keys/buttons through the OS: that too
/// would be input after revocation. The host should tell the user to release any
/// held physical modifiers/buttons when stopping a drag or held key.
pub fn kill_switch() {
    let start = Instant::now();
    global().lock().unwrap_or_else(|e| e.into_inner()).disarm();
    audit("desktop_input_kill_switch", start, &Ok(()));
}

/// Only a trusted host-side consent action may call this. Old controllers stay
/// invalid after a kill; remote messages must never automatically rearm input.
pub fn rearm() -> Result<(), ScreenError> {
    let start = Instant::now();
    let result = (|| {
        access()?;
        let mut gate = global().lock().map_err(|_| failed("input gate poisoned"))?;
        gate.armed = true;
        Ok(())
    })();
    audit("desktop_input_rearm", start, &result);
    result
}

fn click_directions(count: u8) -> Result<impl Iterator<Item = bool>, ScreenError> {
    if !(1..=3).contains(&count) {
        return Err(failed("click count must be between 1 and 3"));
    }
    Ok((0..usize::from(count) * 2).map(|event| event % 2 == 0))
}

// Enigo 0.6.1 aliases macos/mod.rs and win/mod.rs as `platform`; the
// implementation module names, not their source directory names, are the log
// targets. keycodes also logs Unicode key values at TRACE (covered by DEBUG).
const ENIGO_LOG_TARGETS: &[&str] = &[
    "enigo",
    "enigo::platform",
    "enigo::keycodes",
    #[cfg(target_os = "macos")]
    "enigo::platform::macos_impl",
    #[cfg(target_os = "windows")]
    "enigo::platform::win_impl",
];

fn private_logging(enabled: impl Fn(&str) -> bool) -> Result<(), ScreenError> {
    if ENIGO_LOG_TARGETS.iter().any(|target| enabled(target)) {
        return Err(failed(
            "disable enigo debug logging before enabling desktop input",
        ));
    }
    Ok(())
}

fn access() -> Result<(), ScreenError> {
    // Fail closed for all input, including Unicode key events, before Enigo can
    // log its arguments. Rechecked immediately before every native emission.
    private_logging(|target| log::log_enabled!(target: target, log::Level::Debug))?;
    // supported() also rejects a locked macOS session; check the session first
    // so input revocation reports INPUT_FAILED rather than SCREEN_UNSUPPORTED.
    #[cfg(target_os = "macos")]
    if !crate::macos_permissions::desktop_session_active() {
        return Err(failed("desktop session is locked or unavailable"));
    }
    if !supported() {
        return Err(ScreenError::Unsupported);
    }
    #[cfg(target_os = "macos")]
    if !crate::macos_permissions::ensure_desktop_input_access() {
        return Err(ScreenError::InputPermissionRequired);
    }
    Ok(())
}

#[derive(Clone)]
enum Action {
    Pointer(f64, f64),
    Button(MouseButton, bool),
    Click(MouseButton, u8),
    Scroll(i32, i32),
    Key(String, bool, Modifiers),
    Text(String),
}

/// Per-controller revocation barrier, independent of the host consent generation.
/// The optional principal check runs at the last possible point before every
/// native call (including each character of a queued Text request).
struct ControllerToken {
    valid: Mutex<bool>,
    authorized: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
}
impl ControllerToken {
    fn new(authorized: Option<Arc<dyn Fn() -> bool + Send + Sync>>) -> Self {
        Self {
            valid: Mutex::new(true),
            authorized,
        }
    }

    fn emit<T>(&self, call: impl FnOnce() -> T) -> Result<T, ScreenError> {
        let mut valid = self
            .valid
            .lock()
            .map_err(|_| failed("controller token poisoned"))?;
        if !*valid || self.authorized.as_ref().is_some_and(|check| !check()) {
            *valid = false;
            return Err(failed("desktop controller revoked"));
        }
        Ok(call())
    }

    fn invalidate(&self) {
        *self.valid.lock().unwrap_or_else(|e| e.into_inner()) = false;
    }
}

struct Request {
    display: String,
    action: Action,
    start: Instant,
    token: Arc<ControllerToken>,
}

impl Request {
    fn operation(&self) -> &'static str {
        match self.action {
            Action::Pointer(..) => "desktop_input_pointer",
            Action::Button(..) => "desktop_input_button",
            Action::Click(..) => "desktop_input_click",
            Action::Scroll(..) => "desktop_input_scroll",
            Action::Key(..) => "desktop_input_key",
            Action::Text(..) => "desktop_input_text",
        }
    }
}

#[derive(Default)]
struct Pending {
    stopped: bool,
    requests: VecDeque<Request>,
}

#[derive(Default)]
struct Queue {
    pending: Mutex<Pending>,
    wake: Condvar,
}

impl Queue {
    fn stop(&self) {
        let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        pending.stopped = true;
        pending.requests.clear();
        self.wake.notify_all();
    }

    fn push(&self, request: Request) -> Result<(), ScreenError> {
        let mut pending = self
            .pending
            .lock()
            .map_err(|_| failed("input queue poisoned"))?;
        if pending.stopped {
            return Err(failed("input worker stopped"));
        }
        // Only adjacent moves coalesce. Never move a pointer across a button,
        // key, display switch, or scroll barrier (drag ordering must survive).
        if matches!(request.action, Action::Pointer(..)) {
            if let Some(last) = pending.requests.back_mut() {
                if last.display == request.display && matches!(last.action, Action::Pointer(..)) {
                    *last = request;
                    self.wake.notify_one();
                    return Ok(());
                }
            }
        }
        if pending.requests.len() >= QUEUE_CAPACITY {
            return Err(failed("desktop input queue full"));
        }
        pending.requests.push_back(request);
        self.wake.notify_one();
        Ok(())
    }

    fn next(&self) -> Option<Request> {
        let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if pending.stopped {
                return None;
            }
            if let Some(request) = pending.requests.pop_front() {
                return Some(request);
            }
            pending = self.wake.wait(pending).unwrap_or_else(|e| e.into_inner());
        }
    }
}

pub struct InputController {
    queue: Arc<Queue>,
    snapshot: Arc<Vec<DisplayInfo>>,
    generation: u64,
    worker: Option<JoinHandle<()>>,
    token: Arc<ControllerToken>,
}

impl InputController {
    pub fn new() -> Result<Self, ScreenError> {
        Self::with_authorization(None)
    }

    pub fn with_authorization(
        authorized: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
    ) -> Result<Self, ScreenError> {
        access()?;
        let token = Arc::new(ControllerToken::new(authorized));
        let snapshot = Arc::new(list_displays()?);
        let queue = Arc::new(Queue::default());
        let generation = {
            let mut gate = global().lock().map_err(|_| failed("input gate poisoned"))?;
            gate.check(gate.generation)?;
            gate.queues.retain(|q| q.strong_count() != 0);
            gate.queues.push(Arc::downgrade(&queue));
            gate.generation
        };
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            let worker_queue = Arc::clone(&queue);
            let displays = Arc::clone(&snapshot);
            let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
            let worker = std::thread::Builder::new()
                .name("desktop-input".into())
                .spawn(move || {
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        native::run(Arc::clone(&worker_queue), displays, generation, ready_tx);
                    }));
                    if result.is_err() {
                        revoke_generation(generation, &worker_queue);
                    }
                })
                .map_err(|_| failed("could not start desktop input worker"))?;
            let ready = ready_rx
                .recv()
                .map_err(|_| failed("desktop input worker startup failed"));
            if let Err(error) = ready.and_then(|result| result) {
                queue.stop();
                let _ = worker.join();
                return Err(error);
            }
            Ok(Self {
                queue,
                snapshot,
                generation,
                worker: Some(worker),
                token,
            })
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            let _ = (queue, snapshot, generation, token);
            Err(ScreenError::Unsupported)
        }
    }

    fn enqueue(&self, display: &str, action: Action) -> Result<(), ScreenError> {
        let request = Request {
            display: display.to_owned(),
            action,
            start: Instant::now(),
            token: Arc::clone(&self.token),
        };
        let operation = request.operation();
        let start = request.start;
        let result = (|| {
            access()?;
            validate_display(&self.snapshot, &list_displays()?, display)?;
            let gate = global().lock().map_err(|_| failed("input gate poisoned"))?;
            gate.check(self.generation)?;
            self.token.emit(|| self.queue.push(request))?
        })();
        if result.is_err() {
            audit(operation, start, &result);
        }
        result
    }

    pub fn pointer_move(&self, display: &str, u: f64, v: f64) -> Result<(), ScreenError> {
        let info = self
            .snapshot
            .iter()
            .find(|d| d.id == display)
            .ok_or(ScreenError::DisplayNotFound)?;
        coordinates(info, u, v)?;
        self.enqueue(display, Action::Pointer(u, v))
    }

    pub fn button(
        &self,
        display: &str,
        button: MouseButton,
        down: bool,
    ) -> Result<(), ScreenError> {
        self.enqueue(display, Action::Button(button, down))
    }

    pub fn click(&self, display: &str, button: &MouseButton, count: u8) -> Result<(), ScreenError> {
        let _ = click_directions(count)?;
        self.enqueue(display, Action::Click(*button, count))
    }

    /// Wheel units, positive right/down. Large deltas are rejected, not expanded
    /// into an unbounded sequence of native events.
    pub fn scroll(&self, display: &str, dx: i32, dy: i32) -> Result<(), ScreenError> {
        if dx.unsigned_abs() > 120 || dy.unsigned_abs() > 120 {
            return Err(failed("scroll delta out of range"));
        }
        self.enqueue(display, Action::Scroll(dx, dy))
    }

    pub fn key(
        &self,
        display: &str,
        key: &str,
        down: bool,
        modifiers: Modifiers,
    ) -> Result<(), ScreenError> {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        native::parse_key(key)?;
        if key.len() > 32 || key.is_empty() {
            return Err(failed("unknown key"));
        }
        self.enqueue(display, Action::Key(key.to_owned(), down, modifiers))
    }

    pub fn text(&self, display: &str, text: &str) -> Result<(), ScreenError> {
        if text.len() > MAX_TEXT_BYTES || text.contains('\0') {
            return Err(failed("text exceeds input limit or contains NUL"));
        }
        self.enqueue(display, Action::Text(text.to_owned()))
    }
}

impl InputController {
    /// Synchronous, controller-local barrier: flush queued work without revoking
    /// other devices or the host's consent generation.
    pub fn invalidate(&self) {
        let _gate = global().lock().unwrap_or_else(|e| e.into_inner());
        self.queue.stop();
        self.token.invalidate();
    }
}

impl Drop for InputController {
    fn drop(&mut self) {
        self.invalidate();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
pub(crate) struct InputTestProbe {
    queue: Arc<Queue>,
    token: Arc<ControllerToken>,
}
#[cfg(test)]
impl InputTestProbe {
    pub(crate) fn queued(&self) -> usize {
        self.queue.pending.lock().unwrap().requests.len()
    }
    pub(crate) fn emit(&self) -> bool {
        self.token.emit(|| ()).is_ok()
    }
}
#[cfg(test)]
impl InputController {
    /// Exercise the real queue and native-call barrier without injecting OS input.
    pub(crate) fn for_tests(
        authorized: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
    ) -> (Self, InputTestProbe) {
        let queue = Arc::new(Queue::default());
        let token = Arc::new(ControllerToken::new(authorized));
        queue
            .push(Request {
                display: "old-display".into(),
                action: Action::Text("queued text".repeat(100)),
                start: Instant::now(),
                token: token.clone(),
            })
            .unwrap();
        let controller = Self {
            queue: queue.clone(),
            snapshot: Arc::new(vec![]),
            generation: 0,
            worker: None,
            token: token.clone(),
        };
        (controller, InputTestProbe { queue, token })
    }
}

fn same_display(a: &DisplayInfo, b: &DisplayInfo) -> bool {
    a.id == b.id
        && a.name == b.name
        && a.pixel_width == b.pixel_width
        && a.pixel_height == b.pixel_height
        && a.scale == b.scale
        && a.origin_x == b.origin_x
        && a.origin_y == b.origin_y
        && a.is_primary == b.is_primary
}

fn validate_display<'a>(
    snapshot: &'a [DisplayInfo],
    current: &[DisplayInfo],
    id: &str,
) -> Result<&'a DisplayInfo, ScreenError> {
    let selected = snapshot
        .iter()
        .find(|d| d.id == id)
        .ok_or(ScreenError::DisplayNotFound)?;
    // Reject any changed topology, including origins shifted by another monitor.
    if snapshot.len() != current.len()
        || snapshot
            .iter()
            .any(|a| !current.iter().any(|b| same_display(a, b)))
    {
        return Err(ScreenError::DisplayNotFound);
    }
    Ok(selected)
}

/// Native macOS points; origin is already global points, never divide it again.
fn coordinates(display: &DisplayInfo, u: f64, v: f64) -> Result<(i32, i32), ScreenError> {
    map_coordinates(display, u, v, display.scale)
}

fn map_coordinates(
    display: &DisplayInfo,
    u: f64,
    v: f64,
    scale: f64,
) -> Result<(i32, i32), ScreenError> {
    if !u.is_finite()
        || !v.is_finite()
        || !(0.0..=1.0).contains(&u)
        || !(0.0..=1.0).contains(&v)
        || !scale.is_finite()
        || scale <= 0.0
        || display.pixel_width == 0
        || display.pixel_height == 0
    {
        return Err(failed("invalid pointer coordinates or display geometry"));
    }
    let x = (u * f64::from(display.pixel_width))
        .round()
        .min(f64::from(display.pixel_width - 1))
        / scale
        + f64::from(display.origin_x);
    let y = (v * f64::from(display.pixel_height))
        .round()
        .min(f64::from(display.pixel_height - 1))
        / scale
        + f64::from(display.origin_y);
    // Floor keeps a fractional last pixel within the display's logical extent.
    if x < f64::from(i32::MIN)
        || x > f64::from(i32::MAX)
        || y < f64::from(i32::MIN)
        || y > f64::from(i32::MAX)
    {
        return Err(failed("pointer coordinate overflow"));
    }
    Ok((x.floor() as i32, y.floor() as i32))
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
mod native {
    use super::*;
    use enigo::{Axis, Button, Coordinate, Direction, Enigo, Key, Keyboard, Mouse, Settings};

    pub(super) fn parse_key(value: &str) -> Result<Key, ScreenError> {
        let key = match value {
            "Enter" | "Return" => Key::Return,
            "Escape" | "Esc" => Key::Escape,
            "Tab" => Key::Tab,
            "Backspace" => Key::Backspace,
            "Delete" => Key::Delete,
            "Space" => Key::Space,
            "ArrowLeft" | "Left" => Key::LeftArrow,
            "ArrowRight" | "Right" => Key::RightArrow,
            "ArrowUp" | "Up" => Key::UpArrow,
            "ArrowDown" | "Down" => Key::DownArrow,
            "Home" => Key::Home,
            "End" => Key::End,
            "PageUp" => Key::PageUp,
            "PageDown" => Key::PageDown,
            "Shift" => Key::Shift,
            "Control" | "Ctrl" => Key::Control,
            "Alt" | "Option" => Key::Alt,
            "Meta" | "Command" | "Cmd" => Key::Meta,
            "F1" => Key::F1,
            "F2" => Key::F2,
            "F3" => Key::F3,
            "F4" => Key::F4,
            "F5" => Key::F5,
            "F6" => Key::F6,
            "F7" => Key::F7,
            "F8" => Key::F8,
            "F9" => Key::F9,
            "F10" => Key::F10,
            "F11" => Key::F11,
            "F12" => Key::F12,
            _ => {
                let mut chars = value.chars();
                match (chars.next(), chars.next()) {
                    (Some(c), None) if !c.is_control() => Key::Unicode(c),
                    _ => return Err(failed("unknown key")),
                }
            }
        };
        Ok(key)
    }

    fn button(value: MouseButton) -> Button {
        match value {
            MouseButton::Left => Button::Left,
            MouseButton::Right => Button::Right,
            MouseButton::Middle => Button::Middle,
        }
    }

    fn direction(down: bool) -> Direction {
        if down {
            Direction::Press
        } else {
            Direction::Release
        }
    }

    // Reserve a conservative upper bound per Enigo call, including Unicode's
    // two UTF-16 down/up pairs on Windows. Clicks are explicitly split instead
    // of using native click batching. The budget is shared by ALL controllers.
    fn emit(
        queue: &Queue,
        snapshot: &[DisplayInfo],
        generation: u64,
        request: &Request,
        cost: usize,
        call: impl FnOnce() -> enigo::InputResult<()>,
    ) -> Result<(), ScreenError> {
        loop {
            let mut gate = global().lock().map_err(|_| failed("input gate poisoned"))?;
            gate.check(generation)?;
            if queue
                .pending
                .lock()
                .map_err(|_| failed("input queue poisoned"))?
                .stopped
            {
                return Err(failed("input worker stopped"));
            }
            access()?;
            validate_display(snapshot, &list_displays()?, &request.display)?;
            let delay = gate.limiter.delay(Instant::now(), cost);
            if delay.is_zero() {
                let result = match request.token.emit(call) {
                    Ok(result) => result.map_err(|_| failed("native desktop input failed")),
                    Err(error) => {
                        // Principal revocation is controller-local, not a reason
                        // for the worker to disarm every other controller.
                        queue.stop();
                        Err(error)
                    }
                };
                // Timestamp after return: even a slow OS call cannot weaken the
                // rolling-window guarantee. Failed/partial calls consume budget.
                gate.limiter.record(Instant::now(), cost);
                return result;
            }
            drop(gate);
            let pending = queue
                .pending
                .lock()
                .map_err(|_| failed("input queue poisoned"))?;
            if pending.stopped {
                return Err(failed("input worker stopped"));
            }
            let _ = queue
                .wake
                .wait_timeout(pending, delay)
                .map_err(|_| failed("input queue poisoned"))?;
        }
    }

    fn execute(
        enigo: &mut Enigo,
        queue: &Queue,
        snapshot: &[DisplayInfo],
        generation: u64,
        request: &Request,
    ) -> Result<(), ScreenError> {
        let mut send = |cost, call: &mut dyn FnMut(&mut Enigo) -> enigo::InputResult<()>| {
            emit(queue, snapshot, generation, request, cost, || call(enigo))
        };
        match &request.action {
            Action::Pointer(u, v) => {
                let display = snapshot
                    .iter()
                    .find(|d| d.id == request.display)
                    .ok_or(ScreenError::DisplayNotFound)?;
                #[cfg(target_os = "macos")]
                let (x, y) = coordinates(display, *u, *v)?;
                // Windows xcap origins and SendInput coordinates are physical
                // pixels. Do not apply macOS's point conversion on this path.
                #[cfg(target_os = "windows")]
                let (x, y) = map_coordinates(display, *u, *v, 1.0)?;
                send(1, &mut |e| e.move_mouse(x, y, Coordinate::Abs))
            }
            Action::Button(value, down) => {
                send(1, &mut |e| e.button(button(*value), direction(*down)))
            }
            Action::Click(value, count) => {
                for down in click_directions(*count)? {
                    send(1, &mut |e| e.button(button(*value), direction(down)))?;
                }
                Ok(())
            }
            Action::Scroll(dx, dy) => {
                if *dx != 0 {
                    send(1, &mut |e| e.scroll(*dx, Axis::Horizontal))?;
                }
                if *dy != 0 {
                    send(1, &mut |e| e.scroll(*dy, Axis::Vertical))?;
                }
                Ok(())
            }
            Action::Key(value, down, modifiers) => {
                let key = parse_key(value)?;
                let modifiers = [
                    (modifiers.ctrl, Key::Control),
                    (modifiers.alt, Key::Alt),
                    (modifiers.shift, Key::Shift),
                    (modifiers.meta, Key::Meta),
                ];
                for (enabled, modifier) in modifiers {
                    if enabled && modifier != key {
                        send(1, &mut |e| e.key(modifier, Direction::Press))?;
                    }
                }
                send(
                    if matches!(key, Key::Unicode(_)) { 4 } else { 1 },
                    &mut |e| e.key(key, direction(*down)),
                )?;
                for (enabled, modifier) in modifiers.into_iter().rev() {
                    if enabled && modifier != key {
                        send(1, &mut |e| e.key(modifier, Direction::Release))?;
                    }
                }
                Ok(())
            }
            Action::Text(text) => {
                for c in text.chars() {
                    let mut buffer = [0; 4];
                    let character = c.encode_utf8(&mut buffer);
                    send(4, &mut |e| e.text(character))?;
                }
                Ok(())
            }
        }
    }

    pub(super) fn run(
        queue: Arc<Queue>,
        snapshot: Arc<Vec<DisplayInfo>>,
        generation: u64,
        ready: std::sync::mpsc::SyncSender<Result<(), ScreenError>>,
    ) {
        // Enigo must be constructed AND dropped on this thread. Its normal Drop
        // injects releases; explicitly disable that to preserve kill semantics.
        let settings = Settings {
            release_keys_when_dropped: false,
            open_prompt_to_get_permissions: false,
            ..Settings::default()
        };
        let mut enigo = match Enigo::new(&settings) {
            Ok(enigo) => enigo,
            Err(_) => {
                let _ = ready.send(Err(failed("native desktop input unavailable")));
                queue.stop();
                return;
            }
        };
        if ready.send(Ok(())).is_err() {
            queue.stop();
            return;
        }
        while let Some(request) = queue.next() {
            let result = execute(&mut enigo, &queue, &snapshot, generation, &request);
            audit(request.operation(), request.start, &result);
            if result.is_err() {
                revoke_generation(generation, &queue);
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn display() -> DisplayInfo {
        DisplayInfo {
            id: "retina".into(),
            name: "Retina".into(),
            pixel_width: 2880,
            pixel_height: 1800,
            scale: 2.0,
            origin_x: 0,
            origin_y: 0,
            is_primary: true,
        }
    }

    #[test]
    fn retina_pixels_become_points_and_edges_stay_inside() {
        let d = display();
        assert_eq!(coordinates(&d, 0.5, 0.5).unwrap(), (720, 450));
        assert_eq!(coordinates(&d, 1.0, 1.0).unwrap(), (1439, 899));
    }

    #[test]
    fn negative_origin_is_already_global_points() {
        let mut d = display();
        d.origin_x = -1440;
        d.origin_y = -900;
        assert_eq!(coordinates(&d, 0.5, 0.5).unwrap(), (-720, -450));
        assert_eq!(coordinates(&d, 0.0, 0.0).unwrap(), (-1440, -900));
    }

    #[test]
    fn invalid_coordinates_are_rejected_not_clamped() {
        let mut d = display();
        for (u, v) in [
            (-0.01, 0.5),
            (1.01, 0.5),
            (f64::NAN, 0.0),
            (0.0, f64::INFINITY),
        ] {
            assert!(coordinates(&d, u, v).is_err());
        }
        d.scale = 0.0;
        assert!(coordinates(&d, 0.5, 0.5).is_err());
    }

    #[test]
    fn unknown_or_stale_topology_is_rejected() {
        let old = vec![display()];
        assert!(validate_display(&old, &old, "unknown").is_err());
        assert!(validate_display(&old, &[], "retina").is_err());
        let mut changed = display();
        changed.origin_x = -1440;
        assert!(validate_display(&old, &[changed], "retina").is_err());
        assert!(validate_display(&old, &old, "retina").is_ok());
    }

    #[test]
    fn clicks_validate_count_and_budget_each_native_transition() {
        let controller = InputController {
            queue: Arc::new(Queue::default()),
            snapshot: Arc::new(vec![display()]),
            generation: 0,
            worker: None,
            token: Arc::new(ControllerToken::new(None)),
        };
        for count in [0, 4, u8::MAX] {
            assert!(controller
                .click("retina", &MouseButton::Left, count)
                .is_err());
            assert!(click_directions(count).is_err());
        }
        for count in 1..=3 {
            let directions: Vec<_> = click_directions(count).unwrap().collect();
            assert_eq!(directions.len(), usize::from(count) * 2);
            for (index, down) in directions.iter().enumerate() {
                assert_eq!(*down, index % 2 == 0);
            }
        }
        let mut limiter = Limiter::default();
        let now = Instant::now();
        for _ in 0..20 {
            for _ in click_directions(3).unwrap() {
                assert_eq!(limiter.delay(now, 1), Duration::ZERO);
                limiter.record(now, 1);
            }
        }
        assert_eq!(limiter.events.len(), 120);
        assert_eq!(limiter.delay(now, 1), RATE_WINDOW);
    }

    #[test]
    fn enigo_debug_logging_fails_closed_for_every_actual_target() {
        assert!(private_logging(|_| false).is_ok());
        assert!(private_logging(|target| target == "unrelated_application").is_ok());
        for blocked_target in ENIGO_LOG_TARGETS {
            let error = private_logging(|target| target == *blocked_target).unwrap_err();
            assert!(matches!(error, ScreenError::InputFailed(_)));
            if let ScreenError::InputFailed(message) = error {
                assert_eq!(
                    message,
                    "disable enigo debug logging before enabling desktop input"
                );
            }
        }
        #[cfg(target_os = "macos")]
        assert!(ENIGO_LOG_TARGETS.contains(&"enigo::platform::macos_impl"));
        #[cfg(target_os = "windows")]
        assert!(ENIGO_LOG_TARGETS.contains(&"enigo::platform::win_impl"));
    }

    #[test]
    fn rolling_limiter_counts_native_events_not_requests() {
        let mut limiter = Limiter::default();
        let now = Instant::now();
        for _ in 0..30 {
            assert_eq!(limiter.delay(now, 4), Duration::ZERO);
            limiter.record(now, 4);
        }
        assert_eq!(limiter.delay(now, 1), RATE_WINDOW);
        assert_eq!(
            limiter.delay(now + Duration::from_millis(999), 4),
            Duration::from_millis(1)
        );
        assert_eq!(limiter.delay(now + RATE_WINDOW, 4), Duration::ZERO);
        assert!(limiter.events.is_empty());
    }

    #[test]
    fn kill_cancels_queue_and_old_generation_without_native_input() {
        let queue = Arc::new(Queue::default());
        let mut gate = Gate::default();
        assert!(gate.check(0).is_err());
        gate.armed = true;
        let generation = gate.generation;
        gate.queues.push(Arc::downgrade(&queue));
        queue
            .push(Request {
                display: "retina".into(),
                action: Action::Pointer(0.5, 0.5),
                start: Instant::now(),
                token: Arc::new(ControllerToken::new(None)),
            })
            .unwrap();
        gate.disarm();
        assert!(queue.next().is_none());
        assert!(gate.check(generation).is_err());
        gate.armed = true;
        assert!(gate.check(generation).is_err());
        assert!(gate.check(gate.generation).is_ok());
        assert!(queue
            .push(Request {
                display: "retina".into(),
                action: Action::Click(MouseButton::Left, 1),
                start: Instant::now(),
                token: Arc::new(ControllerToken::new(None)),
            })
            .is_err());
    }

    #[test]
    fn public_kill_waits_for_inflight_call_and_stale_worker_cannot_revoke_rearm() {
        use std::sync::mpsc;
        let queue = Arc::new(Queue::default());
        let generation = {
            let mut gate = global().lock().unwrap();
            gate.armed = true;
            gate.queues.push(Arc::downgrade(&queue));
            gate.generation
        };
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let in_flight = std::thread::spawn(move || {
            let gate = global().lock().unwrap();
            gate.check(generation).unwrap();
            // Stand in for a native call, without creating Enigo or an event.
            entered_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        entered_rx.recv().unwrap();
        let (requested_tx, requested_rx) = mpsc::channel();
        let (stopped_tx, stopped_rx) = mpsc::channel();
        let stop = std::thread::spawn(move || {
            requested_tx.send(()).unwrap();
            kill_switch();
            stopped_tx.send(()).unwrap();
        });
        requested_rx.recv().unwrap();
        assert!(stopped_rx.try_recv().is_err());
        release_tx.send(()).unwrap();
        stopped_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        in_flight.join().unwrap();
        stop.join().unwrap();
        {
            let mut gate = global().lock().unwrap();
            assert!(!gate.armed);
            assert!(gate.check(generation).is_err());
            assert!(queue.pending.lock().unwrap().stopped);
            // Emulate a fresh trusted consent without invoking native permission
            // helpers; this is only state-machine verification.
            gate.armed = true;
        }
        revoke_generation(generation, &queue);
        let mut gate = global().lock().unwrap();
        assert!(gate.armed, "old worker must not revoke the new generation");
        assert!(gate.check(generation).is_err());
        gate.disarm();
    }

    #[test]
    fn invalidated_controller_token_blocks_dequeued_work() {
        let (controller, probe) = InputController::for_tests(None);
        let request = controller.queue.next().unwrap();
        assert!(probe.emit());
        controller.invalidate();
        assert_eq!(probe.queued(), 0);
        let emitted = std::sync::atomic::AtomicBool::new(false);
        assert!(request
            .token
            .emit(|| emitted.store(true, std::sync::atomic::Ordering::SeqCst))
            .is_err());
        assert!(!emitted.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[test]
    fn principal_guard_stops_long_text_before_next_native_emit() {
        let authorized = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let check = authorized.clone();
        let (controller, probe) = InputController::for_tests(Some(Arc::new(move || {
            check.load(std::sync::atomic::Ordering::SeqCst)
        })));
        assert!(probe.emit());
        authorized.store(false, std::sync::atomic::Ordering::SeqCst);
        assert!(!probe.emit());
        authorized.store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(!probe.emit(), "scope regrant cannot revive the old token");
        drop(controller);
        assert_eq!(probe.queued(), 0);
    }

    #[test]
    fn pointer_coalescing_preserves_button_barriers_and_bounds() {
        let queue = Queue::default();
        let push = |action| {
            queue.push(Request {
                display: "retina".into(),
                action,
                start: Instant::now(),
                token: Arc::new(ControllerToken::new(None)),
            })
        };
        push(Action::Pointer(0.1, 0.1)).unwrap();
        push(Action::Pointer(0.2, 0.2)).unwrap();
        push(Action::Button(MouseButton::Left, true)).unwrap();
        push(Action::Pointer(0.3, 0.3)).unwrap();
        assert_eq!(queue.pending.lock().unwrap().requests.len(), 3);
        assert!(matches!(
            queue.next().unwrap().action,
            Action::Pointer(0.2, 0.2)
        ));
        for _ in 0..QUEUE_CAPACITY - 2 {
            push(Action::Click(MouseButton::Left, 1)).unwrap();
        }
        assert!(push(Action::Click(MouseButton::Left, 1)).is_err());
        queue.stop();
    }
}
