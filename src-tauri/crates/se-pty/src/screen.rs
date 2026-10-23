//! Host-owned, spawn-time observation. Native code is never called here.
use base64::{engine::general_purpose::STANDARD, Engine};
use se_screen_protocol::{
    Request, Response, GHOSTTY_REVISION, MAX_LINES, MAX_TEXT_BYTES, PROTOCOL_VERSION,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc::{sync_channel, SyncSender},
        Arc, Mutex, OnceLock,
    },
    time::{Duration, Instant},
};
use tokio::sync::oneshot;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TerminalScreenEngine {
    #[default]
    Existing,
    GhosttyVt,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ScreenObservationState {
    Disabled,
    Starting,
    CatchingUp,
    Ready,
    Degraded,
    Unsupported,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ScreenObservationReason {
    HelperMissing,
    IdentityMismatch,
    Overflow,
    Timeout,
    Crash,
    Protocol,
    SizeLimit,
    LegacyHost,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenObservationStatus {
    pub terminal_screen_engine: TerminalScreenEngine,
    pub state: ScreenObservationState,
    pub reason: Option<ScreenObservationReason>,
    pub captured_seq: u64,
    pub applied_seq: u64,
    pub observer_build_id: Option<String>,
}
impl ScreenObservationStatus {
    pub fn disabled() -> Self {
        Self {
            terminal_screen_engine: TerminalScreenEngine::Existing,
            state: ScreenObservationState::Disabled,
            reason: None,
            captured_seq: 0,
            applied_seq: 0,
            observer_build_id: None,
        }
    }
    fn active() -> Self {
        Self {
            terminal_screen_engine: TerminalScreenEngine::GhosttyVt,
            state: ScreenObservationState::Starting,
            ..Self::disabled()
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenReadReply {
    pub status: ScreenObservationStatus,
    pub text: Option<String>,
    pub applied_seq: Option<u64>,
    pub truncated: bool,
}
impl ScreenReadReply {
    fn unavailable(status: ScreenObservationStatus) -> Self {
        Self {
            status,
            text: None,
            applied_seq: None,
            truncated: false,
        }
    }
}

static CONFIG: OnceLock<Option<String>> = OnceLock::new();
static HELPERS: AtomicUsize = AtomicUsize::new(0);
const MAX_QUEUE_BYTES: usize = 4 * 1024 * 1024;
const MAX_QUEUE_FRAMES: usize = 256;
const DEADLINE: Duration = Duration::from_secs(2);
/// Called once by each executable before constructing any PTY manager.
/// No native symbols are called by configuring the parent.
pub fn configure_observer(native_available: bool, build_id: &str) {
    let _ = CONFIG.set(native_available.then(|| build_id.to_owned()));
}

struct PendingRead {
    id: u64,
    barrier: u64,
    sender: oneshot::Sender<ScreenReadReply>,
}
struct State {
    status: ScreenObservationStatus,
    initialized: bool,
    hello: bool,
    outstanding: BTreeMap<u64, usize>,
    bytes: usize,
    progress: Instant,
    read: Option<PendingRead>,
    next_read: u64,
}
struct Inner {
    state: Mutex<State>,
    child: Mutex<Option<Child>>,
    expected_build_id: String,
}
impl Inner {
    fn failed(&self, reason: ScreenObservationReason) {
        let mut state = self.state.lock().unwrap();
        if matches!(
            state.status.state,
            ScreenObservationState::Degraded | ScreenObservationState::Unsupported
        ) {
            return;
        }
        state.status.state = ScreenObservationState::Degraded;
        state.status.reason = Some(reason);
        log::warn!(
            "[screen-observer] degraded reason={reason:?} captured_seq={} applied_seq={}",
            state.status.captured_seq,
            state.status.applied_seq
        );
        state.outstanding.clear();
        state.bytes = 0;
        if let Some(read) = state.read.take() {
            let _ = read
                .sender
                .send(ScreenReadReply::unavailable(state.status.clone()));
        }
        // The watchdog kills/reaps outside this short in-memory gate.
    }
    fn accept(&self, response: Response) {
        let mut state = self.state.lock().unwrap();
        if matches!(
            state.status.state,
            ScreenObservationState::Degraded | ScreenObservationState::Unsupported
        ) {
            return;
        }
        let valid = match response {
            Response::Hello {
                protocol_version,
                observer_build_id,
                ghostty_revision,
            } => {
                if state.hello
                    || protocol_version != PROTOCOL_VERSION
                    || observer_build_id != self.expected_build_id
                    || ghostty_revision != GHOSTTY_REVISION
                {
                    drop(state);
                    self.failed(ScreenObservationReason::IdentityMismatch);
                    return;
                }
                state.hello = true;
                state.status.observer_build_id = Some(observer_build_id);
                true
            }
            Response::Ack { applied_seq } => {
                if !state.hello {
                    false
                } else if !state.initialized {
                    if applied_seq == 0 {
                        state.initialized = true;
                        true
                    } else {
                        false
                    }
                } else if applied_seq != state.status.applied_seq + 1
                    || state.outstanding.keys().next().copied() != Some(applied_seq)
                {
                    false
                } else {
                    let bytes = state.outstanding.remove(&applied_seq).unwrap();
                    state.bytes -= bytes;
                    state.status.applied_seq = applied_seq;
                    true
                }
            }
            Response::Reply {
                request_id,
                applied_seq,
                text,
                truncated,
            } => {
                if text.len() > MAX_TEXT_BYTES {
                    false
                } else if let Some(read) = state.read.take() {
                    if request_id != read.id || applied_seq != read.barrier {
                        state.read = Some(read);
                        false
                    } else {
                        let ready = state.initialized
                            && state.status.captured_seq == applied_seq
                            && state.status.applied_seq == applied_seq;
                        let reply = if ready {
                            ScreenReadReply {
                                status: state.status.clone(),
                                text: Some(text),
                                applied_seq: Some(applied_seq),
                                truncated,
                            }
                        } else {
                            ScreenReadReply::unavailable(state.status.clone())
                        };
                        let _ = read.sender.send(reply);
                        true
                    }
                } else {
                    false
                }
            }
        };
        if !valid {
            drop(state);
            self.failed(ScreenObservationReason::Protocol);
            return;
        }
        state.progress = Instant::now();
        state.status.state = if !state.initialized {
            ScreenObservationState::Starting
        } else if state.status.captured_seq == state.status.applied_seq {
            ScreenObservationState::Ready
        } else {
            ScreenObservationState::CatchingUp
        };
    }
}

/// One per PTY; workers can block only on this child's pipes, never on PTY I/O.
pub struct ScreenObserver {
    inner: Arc<Inner>,
    tx: Option<SyncSender<Request>>,
}
impl ScreenObserver {
    pub fn start(cols: u16, rows: u16) -> Self {
        // Only the explicit native acceptance test can replace its test-harness
        // executable with the separately built production host. Never compiled
        // into a runtime, so production cannot accept a PATH/env helper override.
        #[cfg(test)]
        if let Some(binary) = std::env::var_os("SE_TEST_OBSERVER_BINARY") {
            return Self::start_with(
                cols,
                rows,
                Some(
                    std::env::var("SE_TEST_OBSERVER_BUILD_ID").expect("native test build identity"),
                ),
                Some(binary.into()),
                None,
            );
        }
        Self::start_with(
            cols,
            rows,
            CONFIG.get().cloned().flatten(),
            std::env::current_exe().ok(),
            None,
        )
    }
    fn start_with(
        cols: u16,
        rows: u16,
        build: Option<String>,
        executable: Option<std::path::PathBuf>,
        test_args: Option<Vec<String>>,
    ) -> Self {
        let inner = Arc::new(Inner {
            state: Mutex::new(State {
                status: ScreenObservationStatus::active(),
                initialized: false,
                hello: false,
                outstanding: BTreeMap::new(),
                bytes: 0,
                progress: Instant::now(),
                read: None,
                next_read: 1,
            }),
            child: Mutex::new(None),
            expected_build_id: build.clone().unwrap_or_default(),
        });
        let mut observer = Self {
            inner: inner.clone(),
            tx: None,
        };
        let Some(_) = build else {
            let mut state = inner.state.lock().unwrap();
            state.status.state = ScreenObservationState::Unsupported;
            state.status.reason = Some(ScreenObservationReason::HelperMissing);
            log::info!("[screen-observer] unsupported: native host capability unavailable");
            return observer;
        };
        if !se_screen_protocol::valid_grid(cols, rows) {
            inner.failed(ScreenObservationReason::SizeLimit);
            return observer;
        }
        if HELPERS
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
                (n < 30).then_some(n + 1)
            })
            .is_err()
        {
            inner.failed(ScreenObservationReason::Overflow);
            return observer;
        }
        let child = executable.ok_or(()).and_then(|executable| {
            let mut command = Command::new(executable);
            command
                .env_clear()
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null());
            #[cfg(unix)]
            {
                use std::os::unix::process::CommandExt;
                // POSIX close-on-exec isn't assumed for PTY/plugin-owned descriptors.
                // In the fork child only async-signal-safe fcntl/syscall operations run.
                let max_fd =
                    unsafe { libc::sysconf(libc::_SC_OPEN_MAX) }.clamp(256, 1_048_576) as i32;
                unsafe {
                    command.pre_exec(move || {
                        #[cfg(target_os = "linux")]
                        if libc::syscall(libc::SYS_close_range, 3u32, u32::MAX, 4u32) == 0 {
                            return Ok(());
                        }
                        #[allow(unreachable_code)]
                        {
                            for fd in 3..max_fd {
                                libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
                            }
                            Ok(())
                        }
                    });
                }
            }
            if let Some(args) = test_args {
                command.args(args);
            } else {
                command.arg("--ghostty-vt-observer");
            }
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                command.creation_flags(0x08000000);
            }
            command.spawn().map_err(|_| ())
        });
        let Ok(mut child) = child else {
            HELPERS.fetch_sub(1, Ordering::SeqCst);
            inner.failed(ScreenObservationReason::HelperMissing);
            return observer;
        };
        let mut input = child.stdin.take().unwrap();
        let mut output = child.stdout.take().unwrap();
        *inner.child.lock().unwrap() = Some(child);
        let (tx, rx) = sync_channel(MAX_QUEUE_FRAMES);
        // Init is queued before this observer is published / reader is started.
        tx.try_send(Request::Init { cols, rows }).unwrap();
        observer.tx = Some(tx);
        let writer = inner.clone();
        std::thread::spawn(move || loop {
            let frame = match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(frame) => frame,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    if matches!(
                        writer.state.lock().unwrap().status.state,
                        ScreenObservationState::Degraded
                    ) {
                        break;
                    }
                    continue;
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            };
            if matches!(
                writer.state.lock().unwrap().status.state,
                ScreenObservationState::Degraded
            ) {
                break;
            }
            if se_screen_protocol::write_frame(&mut input, &frame).is_err() {
                writer.failed(ScreenObservationReason::Crash);
                break;
            }
        });
        let reader = inner.clone();
        std::thread::spawn(move || loop {
            match se_screen_protocol::read_frame(&mut output) {
                Ok(frame) => reader.accept(frame),
                Err(error) => {
                    reader.failed(if error.kind() == std::io::ErrorKind::InvalidData {
                        ScreenObservationReason::Protocol
                    } else {
                        ScreenObservationReason::Crash
                    });
                    break;
                }
            }
        });
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_millis(25));
            let overdue = {
                let state = inner.state.lock().unwrap();
                (!state.initialized || !state.outstanding.is_empty() || state.read.is_some())
                    && state.progress.elapsed() >= DEADLINE
            };
            if overdue {
                inner.failed(ScreenObservationReason::Timeout);
            }
            let degraded = matches!(
                inner.state.lock().unwrap().status.state,
                ScreenObservationState::Degraded
            );
            let exited = {
                let mut child = inner.child.lock().unwrap();
                child
                    .as_mut()
                    .and_then(|child| child.try_wait().ok())
                    .flatten()
                    .is_some()
            };
            if degraded || exited {
                if exited && !degraded {
                    inner.failed(ScreenObservationReason::Crash);
                }
                if let Some(mut child) = inner.child.lock().unwrap().take() {
                    let _ = child.kill();
                    let deadline = Instant::now() + Duration::from_millis(200);
                    while child.try_wait().ok().flatten().is_none() && Instant::now() < deadline {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                }
                HELPERS.fetch_sub(1, Ordering::SeqCst);
                break;
            }
        });
        observer
    }
    pub fn status(&self) -> ScreenObservationStatus {
        self.inner.state.lock().unwrap().status.clone()
    }
    fn enqueue(&self, make: impl FnOnce(u64) -> Request, bytes: usize) {
        let mut state = self.inner.state.lock().unwrap();
        if matches!(
            state.status.state,
            ScreenObservationState::Degraded | ScreenObservationState::Unsupported
        ) {
            return;
        }
        if state.outstanding.len()
            + usize::from(!state.initialized)
            + usize::from(state.read.is_some())
            >= MAX_QUEUE_FRAMES
            || state.bytes.saturating_add(bytes) > MAX_QUEUE_BYTES
        {
            drop(state);
            self.inner.failed(ScreenObservationReason::Overflow);
            return;
        }
        let seq = state.status.captured_seq + 1;
        if self
            .tx
            .as_ref()
            .is_none_or(|tx| tx.try_send(make(seq)).is_err())
        {
            drop(state);
            self.inner.failed(ScreenObservationReason::Overflow);
            return;
        }
        if state.outstanding.is_empty() && state.read.is_none() {
            state.progress = Instant::now();
        }
        state.status.captured_seq = seq;
        state.outstanding.insert(seq, bytes);
        state.bytes += bytes;
        if state.initialized {
            state.status.state = ScreenObservationState::CatchingUp;
        }
    }
    pub fn output(&self, bytes: &[u8]) {
        if !bytes.is_empty() {
            self.enqueue(
                |event_seq| Request::Output {
                    event_seq,
                    data_base64: STANDARD.encode(bytes),
                },
                bytes.len().saturating_mul(4) / 3 + 128,
            );
        }
    }
    pub fn resize(&self, cols: u16, rows: u16, _cell_width: u32, _cell_height: u32) {
        if !se_screen_protocol::valid_grid(cols, rows) {
            self.inner.failed(ScreenObservationReason::SizeLimit);
            return;
        }
        self.enqueue(
            |event_seq| Request::Resize {
                event_seq,
                cols,
                rows,
            },
            128,
        );
    }
    pub async fn read(&self, max_lines: usize) -> Result<ScreenReadReply, String> {
        let receiver = {
            let mut state = self.inner.state.lock().unwrap();
            if state.status.state != ScreenObservationState::Ready {
                return Ok(ScreenReadReply::unavailable(state.status.clone()));
            }
            if state.read.is_some() {
                return Err("screen observation busy".into());
            }
            let (sender, receiver) = oneshot::channel();
            let id = state.next_read;
            state.next_read += 1;
            let barrier = state.status.captured_seq;
            if self.tx.as_ref().is_none_or(|tx| {
                tx.try_send(Request::Read {
                    request_id: id,
                    barrier_seq: barrier,
                    max_lines: max_lines.clamp(1, MAX_LINES),
                })
                .is_err()
            }) {
                drop(state);
                self.inner.failed(ScreenObservationReason::Overflow);
                return Ok(ScreenReadReply::unavailable(self.status()));
            }
            state.read = Some(PendingRead {
                id,
                barrier,
                sender,
            });
            state.progress = Instant::now();
            receiver
        };
        match tokio::time::timeout(DEADLINE + Duration::from_millis(50), receiver).await {
            Ok(Ok(reply)) => Ok(reply),
            _ => {
                self.inner.failed(ScreenObservationReason::Timeout);
                Ok(ScreenReadReply::unavailable(self.status()))
            }
        }
    }
    pub fn stop(&self) {
        self.inner.failed(ScreenObservationReason::Crash);
    }
}
impl Drop for ScreenObserver {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    const FIXTURE: &str = r#"
import os,sys,json,struct,time,base64
mode=sys.argv[1]
def send(x):
 b=json.dumps(x,separators=(',',':')).encode();sys.stdout.buffer.write(struct.pack('>I',len(b))+b);sys.stdout.buffer.flush()
def read():
 h=sys.stdin.buffer.read(4)
 if len(h)!=4:sys.exit(0)
 return json.loads(sys.stdin.buffer.read(struct.unpack('>I',h)[0]))
if mode=='hang': time.sleep(20);sys.exit(0)
send({'type':'hello','protocolVersion':1,'observerBuildId':'wrong' if mode=='identity' else 'fixture','ghosttyRevision':'44f2a44'})
assert read()['type']=='init'
send({'type':'ack','appliedSeq':0})
if mode=='crash':os._exit(71)
if mode=='stall':time.sleep(20)
seq=0;trace=[]
while True:
 x=read();kind=x['type']
 if kind=='output':
  assert x['eventSeq']==seq+1;seq+=1;trace.append('O:'+base64.b64decode(x['dataBase64']).decode(errors='replace'));send({'type':'ack','appliedSeq':seq})
 elif kind=='resize':
  assert x['eventSeq']==seq+1;seq+=1;trace.append('R:'+str(x['cols'])+'x'+str(x['rows']));send({'type':'ack','appliedSeq':seq})
 elif kind=='read':
  assert x['barrierSeq']==seq
  if mode=='slow-read':time.sleep(.2)
  send({'type':'reply','requestId':x['requestId'],'appliedSeq':seq,'text':'|'.join(trace),'truncated':False})
"#;
    fn fixture(mode: &str) -> Arc<ScreenObserver> {
        Arc::new(ScreenObserver::start_with(
            80,
            24,
            Some("fixture".into()),
            Some("/usr/bin/python3".into()),
            Some(vec!["-u".into(), "-c".into(), FIXTURE.into(), mode.into()]),
        ))
    }
    async fn until(observer: &ScreenObserver, expected: ScreenObservationState) {
        let deadline = Instant::now() + Duration::from_secs(4);
        loop {
            let status = observer.status();
            if status.state == expected {
                return;
            }
            assert!(Instant::now() < deadline, "{:?}", status);
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
    #[test]
    fn legacy_mode_is_default_and_unknown_modes_rejected() {
        assert_eq!(
            serde_json::from_str::<crate::manager::SpawnOptions>("{}")
                .unwrap()
                .terminal_screen_engine,
            TerminalScreenEngine::Existing
        );
        assert!(serde_json::from_str::<crate::manager::SpawnOptions>(
            r#"{"terminalScreenEngine":"browser-engine"}"#
        )
        .is_err());
    }
    #[test]
    fn unavailable_or_missing_helper_never_reports_ready() {
        let unavailable = ScreenObserver::start_with(80, 24, None, None, None);
        assert_eq!(
            unavailable.status().state,
            ScreenObservationState::Unsupported
        );
        let missing = ScreenObserver::start_with(
            80,
            24,
            Some("fixture".into()),
            Some("/missing/observer".into()),
            None,
        );
        assert_eq!(missing.status().state, ScreenObservationState::Degraded);
        assert_eq!(
            missing.status().reason,
            Some(ScreenObservationReason::HelperMissing)
        );
    }
    #[tokio::test]
    async fn ordered_output_resize_and_idle_read_barrier() {
        let observer = fixture("normal");
        until(&observer, ScreenObservationState::Ready).await;
        // Idle is not a reason to close stdin or lose later screen reads.
        tokio::time::sleep(Duration::from_millis(150)).await;
        observer.output(b"one");
        observer.resize(40, 10, 8, 16);
        observer.output(b"two");
        until(&observer, ScreenObservationState::Ready).await;
        let reply = observer.read(200).await.unwrap();
        assert_eq!(reply.text.as_deref(), Some("O:one|R:40x10|O:two"));
        assert_eq!(reply.applied_seq, Some(3));
        assert_eq!(reply.status.captured_seq, reply.status.applied_seq);
    }
    #[tokio::test]
    async fn process_crash_and_identity_mismatch_are_permanent() {
        for (mode, reason) in [
            ("crash", ScreenObservationReason::Crash),
            ("identity", ScreenObservationReason::IdentityMismatch),
        ] {
            let observer = fixture(mode);
            until(&observer, ScreenObservationState::Degraded).await;
            assert_eq!(observer.status().reason, Some(reason));
            observer.output(b"still PTY data");
            assert!(observer.read(20).await.unwrap().text.is_none());
            assert_eq!(observer.status().state, ScreenObservationState::Degraded);
        }
    }
    #[tokio::test]
    async fn hung_child_degrades_without_blocking_producer() {
        let observer = fixture("hang");
        let before = Instant::now();
        observer.output(b"unblocked");
        assert!(before.elapsed() < Duration::from_millis(100));
        until(&observer, ScreenObservationState::Degraded).await;
        assert_eq!(
            observer.status().reason,
            Some(ScreenObservationReason::Timeout)
        );
    }
    #[tokio::test]
    async fn queue_overflow_is_nonblocking_and_does_not_restart() {
        let observer = fixture("stall");
        until(&observer, ScreenObservationState::Ready).await;
        let before = Instant::now();
        for _ in 0..600 {
            observer.output(&[b'x'; 8192]);
        }
        assert!(before.elapsed() < Duration::from_secs(1));
        assert_eq!(observer.status().state, ScreenObservationState::Degraded);
        assert_eq!(
            observer.status().reason,
            Some(ScreenObservationReason::Overflow)
        );
        assert!(observer.read(20).await.unwrap().text.is_none());
    }
    #[tokio::test]
    async fn concurrent_output_invalidates_read_and_single_read_is_enforced() {
        let observer = fixture("slow-read");
        until(&observer, ScreenObservationState::Ready).await;
        let reader = observer.clone();
        let task = tokio::spawn(async move { reader.read(30).await });
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(observer.read(20).await.is_err());
        observer.output(b"later");
        let reply = task.await.unwrap().unwrap();
        assert!(reply.text.is_none());
        until(&observer, ScreenObservationState::Ready).await;
    }

    /// The acceptance harness builds the real host first, then runs this test
    /// explicitly. Default developer tests stay independent of Zig/native.
    #[tokio::test]
    #[ignore = "requires a real native host binary via SE_TEST_OBSERVER_BINARY and SE_TEST_OBSERVER_BUILD_ID"]
    async fn native_self_exec_supervisor_screen_and_crash_isolation() {
        let binary =
            std::env::var_os("SE_TEST_OBSERVER_BINARY").expect("native host binary is required");
        let build_id = std::env::var("SE_TEST_OBSERVER_BUILD_ID")
            .expect("native host build identity is required");
        let observer =
            ScreenObserver::start_with(80, 24, Some(build_id), Some(binary.into()), None);
        until(&observer, ScreenObservationState::Ready).await;
        observer.output(b"\x1b[31mprogress 10%\x1b[0m\rprogress 99%");
        observer.resize(100, 30, 8, 16);
        observer.output("\r\n中文".as_bytes());
        until(&observer, ScreenObservationState::Ready).await;
        let reply = observer.read(30).await.unwrap();
        assert!(reply.text.as_deref().unwrap().contains("progress 99%"));
        assert!(reply.text.as_deref().unwrap().contains("中文"));
        assert_eq!(reply.applied_seq, Some(3));
        assert_eq!(reply.status.applied_seq, reply.status.captured_seq);

        // Measure the bypass/enqueue boundary without asserting an arbitrary
        // performance threshold. Full PTY behavior is covered in manager tests.
        let chunk = [b'x'; 4096];
        let baseline = Instant::now();
        for _ in 0..128 {
            std::hint::black_box(&chunk);
        }
        let disabled_ns = baseline.elapsed().as_nanos();
        let enabled = Instant::now();
        for _ in 0..128 {
            observer.output(&chunk);
        }
        let enqueue_ns = enabled.elapsed().as_nanos();
        until(&observer, ScreenObservationState::Ready).await;
        eprintln!(
            "GHOSTTY_OBSERVATION_SAMPLE bytes=524288 disabled_boundary_ns={disabled_ns} enabled_enqueue_ns={enqueue_ns} drain_ns={}",
            enabled.elapsed().as_nanos()
        );

        let pid = {
            let mut child = observer.inner.child.lock().unwrap();
            let child = child.as_mut().expect("real observer child");
            let pid = child.id();
            child.kill().unwrap();
            pid
        };
        until(&observer, ScreenObservationState::Degraded).await;
        assert_eq!(
            observer.status().reason,
            Some(ScreenObservationReason::Crash)
        );
        observer.output(b"PTY output continues after native child death");
        assert!(observer.read(30).await.unwrap().text.is_none());
        let deadline = Instant::now() + Duration::from_secs(2);
        while observer.inner.child.lock().unwrap().is_some() {
            assert!(Instant::now() < deadline, "observer {pid} was not reaped");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}
