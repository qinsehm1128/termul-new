//! Host-owned process supervision. Broker entry points only mutate bounded in-memory
//! state; spawning, pipe IO, backoff and graceful termination run on Tokio tasks.
//!
//! This is lifecycle management, not a sandbox: native plugins (and their children)
//! retain OS privileges. On Unix, descendants which deliberately leave the process
//! group are outside the group-cleanup guarantee.
use se_plugin_protocol::*;
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::{Child, Command},
    sync::watch,
    task::JoinHandle,
    time::{sleep, timeout, Instant},
};

type Result<T> = std::result::Result<T, PluginError>;
const STOP_GRACE: Duration = Duration::from_millis(500);
const BACKOFF_BASE: Duration = Duration::from_millis(100);
const MAX_RESTARTS: u32 = 5;
const STABLE_RUN: Duration = Duration::from_secs(60);
const LOG_LINES_PER_STREAM: usize = 250;
// Including worst-case JSON escaping, both rings fit MAX_JSON_BYTES.
const LOG_BYTES_PER_STREAM: usize = 16 * 1024;
const LOG_LINE_BYTES: usize = 4096;

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(MAX_SAFE_INTEGER as u128) as u64
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Running,
    Stopped,
    Revoked,
}
#[derive(Clone, Copy)]
struct Intent {
    mode: Mode,
    revision: u64,
}

#[derive(Default)]
struct Ring {
    lines: VecDeque<(u64, HostProcessLogLine)>,
    bytes: usize,
    truncated: bool,
}
impl Ring {
    fn push(&mut self, sequence: u64, line: HostProcessLogLine) {
        self.bytes += line.text.len();
        self.lines.push_back((sequence, line));
        while self.lines.len() > LOG_LINES_PER_STREAM || self.bytes > LOG_BYTES_PER_STREAM {
            if let Some((_, line)) = self.lines.pop_front() {
                self.bytes -= line.text.len();
                self.truncated = true;
            }
        }
    }
}
struct ProcessData {
    status: HostProcessStatus,
    stdout: Ring,
    stderr: Ring,
    sequence: u64,
}
struct Entry {
    intent: watch::Sender<Intent>,
    data: Mutex<ProcessData>,
    changed: Arc<AtomicBool>,
}
impl Entry {
    fn update(&self, revision: u64, f: impl FnOnce(&mut HostProcessStatus)) {
        // Serialize a status transition with a broker intent update. A late exit
        // from the previous run must not overwrite a new start/stop request.
        let intent = self.intent.borrow();
        if intent.revision != revision {
            return;
        }
        let mut data = self.data.lock().unwrap();
        f(&mut data.status);
        self.changed.store(true, Ordering::Release);
    }
    fn append(&self, stream: HostProcessStream, bytes: &[u8], split: bool) {
        let mut text = String::from_utf8_lossy(bytes).into_owned();
        // Invalid UTF-8 can expand; bound the retained UTF-8 bytes as well.
        if text.len() > LOG_LINE_BYTES {
            let mut end = LOG_LINE_BYTES;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            text.truncate(end);
        }
        let mut data = self.data.lock().unwrap();
        let sequence = data.sequence;
        data.sequence = data.sequence.saturating_add(1);
        let ring = match stream {
            HostProcessStream::Stdout => &mut data.stdout,
            HostProcessStream::Stderr => &mut data.stderr,
        };
        ring.truncated |= split || text.len() < bytes.len();
        ring.push(
            sequence,
            HostProcessLogLine {
                stream,
                at: now_ms(),
                text,
            },
        );
        self.changed.store(true, Ordering::Release);
    }
    fn set_intent(&self, mode: Mode) {
        self.intent.send_modify(|intent| {
            intent.mode = mode;
            intent.revision = intent.revision.wrapping_add(1);
            let mut data = self.data.lock().unwrap();
            data.status.state = if mode == Mode::Running {
                HostProcessState::Starting
            } else {
                HostProcessState::Stopped
            };
            // The task still owns the old PID until graceful cleanup completes.
            if mode == Mode::Running {
                data.status.exit_code = None;
            }
            self.changed.store(true, Ordering::Release);
        });
    }
}
struct Activation {
    generation: String,
    entries: BTreeMap<String, Arc<Entry>>,
}
/// Identity is sampled from the kernel, not the manifest or wall-clock spawn time.
/// Linux start ticks are scoped to a boot to prevent matches across machine reboots.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct ProcessIdentity {
    executable: PathBuf,
    start_time: String,
}
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct LedgerEntry {
    plugin_id: String,
    process_id: String,
    pid: u32,
    pgid: u32,
    #[serde(flatten)]
    identity: ProcessIdentity,
}

fn ledger_path(profile: &Path) -> PathBuf {
    profile.join("plugins/managed-processes.json")
}

fn read_ledger(path: &Path) -> std::io::Result<Vec<LedgerEntry>> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(std::io::Error::other),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error),
    }
}

fn write_ledger<'a>(
    path: &Path,
    entries: impl Iterator<Item = &'a LedgerEntry>,
) -> std::io::Result<()> {
    use std::io::Write;
    let parent = path.parent().expect("ledger has a parent");
    std::fs::create_dir_all(parent)?;
    let bytes = serde_json::to_vec(&entries.collect::<Vec<_>>())?;
    let temp = path.with_extension("json.tmp");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temp)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    drop(file);
    // Windows has no Unix process-group recovery; replacing its best-effort ledger
    // may require removing the old destination first.
    #[cfg(windows)]
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    std::fs::rename(temp, path)?;
    #[cfg(unix)]
    std::fs::File::open(parent)?.sync_all()?;
    Ok(())
}

#[cfg(target_os = "linux")]
fn process_identity(pid: u32) -> std::io::Result<ProcessIdentity> {
    fn start(pid: u32) -> std::io::Result<String> {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"))?;
        // comm (field 2) may contain spaces and parentheses. Field 22 is starttime.
        stat.rsplit_once(')')
            .and_then(|(_, rest)| rest.split_whitespace().nth(19))
            .map(str::to_owned)
            .ok_or_else(|| std::io::Error::other("invalid /proc stat"))
    }
    let before = start(pid)?;
    let executable = std::fs::read_link(format!("/proc/{pid}/exe"))?;
    if start(pid)? != before {
        return Err(std::io::Error::other(
            "process changed while reading identity",
        ));
    }
    let boot = std::fs::read_to_string("/proc/sys/kernel/random/boot_id")?;
    Ok(ProcessIdentity {
        executable,
        start_time: format!("{}:{before}", boot.trim()),
    })
}

#[cfg(target_os = "macos")]
fn process_identity(pid: u32) -> std::io::Result<ProcessIdentity> {
    use std::os::unix::ffi::OsStrExt;
    fn start(pid: u32) -> std::io::Result<(u64, u64)> {
        let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::uninit();
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
        // SAFETY: the writable buffer has exactly the size passed to proc_pidinfo.
        let count = unsafe {
            libc::proc_pidinfo(
                pid as i32,
                libc::PROC_PIDTBSDINFO,
                0,
                info.as_mut_ptr().cast(),
                size,
            )
        };
        if count != size {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: proc_pidinfo initialized the entire proc_bsdinfo above.
        let info = unsafe { info.assume_init() };
        Ok((info.pbi_start_tvsec, info.pbi_start_tvusec))
    }
    let before = start(pid)?;
    let mut path = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: path is writable for the specified buffer size.
    let count =
        unsafe { libc::proc_pidpath(pid as i32, path.as_mut_ptr().cast(), path.len() as u32) };
    if count <= 0 {
        return Err(std::io::Error::last_os_error());
    }
    let end = path.iter().position(|&b| b == 0).unwrap_or(count as usize);
    let executable = PathBuf::from(std::ffi::OsStr::from_bytes(&path[..end]));
    if start(pid)? != before {
        return Err(std::io::Error::other(
            "process changed while reading identity",
        ));
    }
    Ok(ProcessIdentity {
        executable,
        start_time: format!("{}:{}", before.0, before.1),
    })
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn process_identity(_pid: u32) -> std::io::Result<ProcessIdentity> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "process identity unavailable",
    ))
}

#[cfg(unix)]
fn in_group(pid: u32, pgid: u32) -> bool {
    // SAFETY: getpgid only queries the kernel; reject unsafe signal targets first.
    pid > 1
        && pgid > 1
        && pid <= i32::MAX as u32
        && unsafe { libc::getpgid(pid as i32) } == pgid as i32
}
#[cfg(not(unix))]
fn in_group(_pid: u32, _pgid: u32) -> bool {
    false
}

fn identity_matches(pid: u32, pgid: u32, identity: &ProcessIdentity) -> bool {
    in_group(pid, pgid) && process_identity(pid).is_ok_and(|current| current == *identity)
}

// Snapshot surviving group members before TERM. If the leader exits during grace,
// an identity-checked survivor still anchors the PGID before escalation. Without
// this check, a recycled PGID could target an unrelated process group.
#[cfg(target_os = "linux")]
fn group_pids(_pgid: u32) -> Vec<u32> {
    std::fs::read_dir("/proc")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| entry.file_name().to_str()?.parse().ok())
        .collect()
}
#[cfg(target_os = "macos")]
fn group_pids(pgid: u32) -> Vec<u32> {
    // PROC_PGRP_ONLY = 2, from libproc.h (not exported by libc).
    // SAFETY: a null buffer with size zero queries the required byte count.
    let size = unsafe { libc::proc_listpids(2, pgid, std::ptr::null_mut(), 0) };
    if size <= 0 {
        return Vec::new();
    }
    let mut pids = vec![0u32; size as usize / std::mem::size_of::<u32>() + 32];
    // SAFETY: the buffer is writable for the byte size passed to libproc.
    let count = unsafe {
        libc::proc_listpids(
            2,
            pgid,
            pids.as_mut_ptr().cast(),
            std::mem::size_of_val(pids.as_slice()) as i32,
        )
    };
    pids.truncate(count.max(0) as usize / std::mem::size_of::<u32>());
    pids
}
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn group_pids(pgid: u32) -> Vec<u32> {
    vec![pgid]
}

/// Must run under the profile's Store lock, before any plugin is loaded/spawned.
pub(crate) async fn reap_orphans(profile: &Path) -> std::io::Result<()> {
    let path = ledger_path(profile);
    let entries = read_ledger(&path)?;
    for entry in entries {
        // We only create isolated groups led by the child. Never trust a corrupt
        // ledger to direct a signal at Core, its caller, or another process group.
        if entry.pid != entry.pgid || !identity_matches(entry.pid, entry.pgid, &entry.identity) {
            log::info!("managed process reap skipped (absent/identity mismatch): plugin={} process={} pid={} pgid={}", entry.plugin_id, entry.process_id, entry.pid, entry.pgid);
            continue;
        }
        let mut witnesses = vec![(entry.pid, entry.identity.clone())];
        for pid in group_pids(entry.pgid) {
            if in_group(pid, entry.pgid) {
                if let Ok(identity) = process_identity(pid) {
                    witnesses.push((pid, identity));
                }
            }
        }
        if !identity_matches(entry.pid, entry.pgid, &entry.identity) {
            log::info!(
                "managed process reap skipped (identity changed): pid={}",
                entry.pid
            );
            continue;
        }
        log::info!(
            "managed process reap SIGTERM: plugin={} process={} pid={} pgid={}",
            entry.plugin_id,
            entry.process_id,
            entry.pid,
            entry.pgid
        );
        signal_group(entry.pgid, false);
        sleep(STOP_GRACE).await;
        if witnesses
            .iter()
            .any(|(pid, identity)| identity_matches(*pid, entry.pgid, identity))
        {
            log::info!("managed process reap SIGKILL: pgid={}", entry.pgid);
            signal_group(entry.pgid, true);
        } else {
            log::info!(
                "managed process reap finished (no matching survivor): pgid={}",
                entry.pgid
            );
        }
    }
    write_ledger(&path, std::iter::empty())?;
    log::info!("managed process startup ledger cleared: {}", path.display());
    Ok(())
}

struct Lifecycle {
    // The lock closes the race between spawn registration and manager Drop and
    // serializes atomic ledger publications from independent supervisors.
    state: Mutex<LifecycleState>,
    ledger: PathBuf,
}
#[derive(Default)]
struct LifecycleState {
    closed: bool,
    groups: BTreeSet<u32>,
    records: BTreeMap<u32, LedgerEntry>,
}

/// A generation-scoped, synchronous facade over asynchronous process supervisors.
/// `activate` must be called inside a Tokio runtime with IO/time enabled.
/// No facade operation waits for process exit or performs filesystem IO.
pub(crate) struct ProcessManager {
    profile: PathBuf,
    active: Mutex<BTreeMap<String, Activation>>,
    changed: Arc<AtomicBool>,
    lifecycle: Arc<Lifecycle>,
}
impl ProcessManager {
    pub(crate) fn new(profile: PathBuf) -> Self {
        let ledger = ledger_path(&profile);
        Self {
            profile,
            active: Mutex::new(BTreeMap::new()),
            changed: Arc::new(AtomicBool::new(false)),
            lifecycle: Arc::new(Lifecycle {
                state: Mutex::new(LifecycleState::default()),
                ledger,
            }),
        }
    }

    pub(crate) fn activate(
        &self,
        id: &str,
        generation: &str,
        package_dir: Option<PathBuf>,
        contributions: &[ProcessContribution],
    ) -> Result<()> {
        let runtime = tokio::runtime::Handle::try_current().map_err(|_| {
            PluginError::new(
                PLUGIN_CORE_UNAVAILABLE,
                "process supervision needs a Tokio runtime",
            )
        })?;
        // Plugin IDs become profile-data path components. Never trust an unchecked
        // caller even though manifests are normally validated before activation.
        if !is_plugin_id(id) || contributions.len() > MAX_MANAGED_PROCESSES {
            return Err(PluginError::new(
                PLUGIN_INVALID_MANIFEST,
                "invalid process activation",
            ));
        }
        let mut seen = BTreeSet::new();
        for contribution in contributions {
            validate_process_contribution(contribution)?;
            if !seen.insert(&contribution.id) {
                return Err(PluginError::new(
                    PLUGIN_INVALID_MANIFEST,
                    "duplicate process id",
                ));
            }
        }
        let mut active = self.active.lock().unwrap();
        if let Some(previous) = active.get(id) {
            if previous.generation == generation {
                return Ok(()); // Repeated activation never duplicates supervisors.
            }
        }
        if let Some(previous) = active.remove(id) {
            for entry in previous.entries.values() {
                entry.set_intent(Mode::Revoked);
            }
        }
        let mut entries = BTreeMap::new();
        for contribution in contributions {
            let mode = if contribution.autostart {
                Mode::Running
            } else {
                Mode::Stopped
            };
            let (intent, receiver) = watch::channel(Intent { mode, revision: 0 });
            let entry = Arc::new(Entry {
                intent,
                data: Mutex::new(ProcessData {
                    status: HostProcessStatus {
                        state: if contribution.autostart {
                            HostProcessState::Starting
                        } else {
                            HostProcessState::Stopped
                        },
                        pid: None,
                        exit_code: None,
                        restarts: 0,
                        started_at: None,
                    },
                    stdout: Ring::default(),
                    stderr: Ring::default(),
                    sequence: 0,
                }),
                changed: self.changed.clone(),
            });
            runtime.spawn(supervise(
                entry.clone(),
                receiver,
                contribution.clone(),
                package_dir.clone(),
                self.profile.join("plugins").join("data").join(id),
                self.lifecycle.clone(),
            ));
            entries.insert(contribution.id.clone(), entry);
        }
        active.insert(
            id.to_owned(),
            Activation {
                generation: generation.to_owned(),
                entries,
            },
        );
        self.changed.store(true, Ordering::Release);
        Ok(())
    }

    pub(crate) fn revoke(&self, id: &str, generation: &str) {
        let mut active = self.active.lock().unwrap();
        if active.get(id).is_some_and(|v| v.generation == generation) {
            if let Some(activation) = active.remove(id) {
                for entry in activation.entries.values() {
                    entry.set_intent(Mode::Revoked);
                }
            }
            self.changed.store(true, Ordering::Release);
        }
    }

    fn with_activation<T>(
        &self,
        id: &str,
        generation: &str,
        f: impl FnOnce(&Activation) -> Result<T>,
    ) -> Result<T> {
        let active = self.active.lock().unwrap();
        let activation = active
            .get(id)
            .ok_or_else(|| PluginError::new(PLUGIN_DISABLED, "plugin processes are inactive"))?;
        if activation.generation != generation {
            return Err(PluginError::new(
                PLUGIN_STALE_GENERATION,
                "stale process generation",
            ));
        }
        f(activation)
    }

    pub(crate) fn list(
        &self,
        id: &str,
        generation: &str,
    ) -> Result<BTreeMap<String, HostProcessStatus>> {
        self.with_activation(id, generation, |activation| {
            Ok(activation
                .entries
                .iter()
                .map(|(name, entry)| (name.clone(), entry.data.lock().unwrap().status.clone()))
                .collect())
        })
    }

    pub(crate) fn logs(
        &self,
        id: &str,
        generation: &str,
        process: &str,
        tail: u32,
    ) -> Result<HostProcessLogsReply> {
        if tail > MAX_PROCESS_LOG_TAIL {
            return Err(PluginError::new(
                PLUGIN_INVALID_REQUEST,
                "process log tail exceeds limit",
            ));
        }
        self.with_activation(id, generation, |activation| {
            let entry = process_entry(activation, process)?;
            let data = entry.data.lock().unwrap();
            let mut lines: Vec<_> = data
                .stdout
                .lines
                .iter()
                .chain(data.stderr.lines.iter())
                .collect();
            lines.sort_unstable_by_key(|(sequence, _)| *sequence);
            let skip = lines.len().saturating_sub(tail as usize);
            Ok(HostProcessLogsReply {
                truncated: data.stdout.truncated || data.stderr.truncated || skip > 0,
                lines: lines
                    .into_iter()
                    .skip(skip)
                    .map(|(_, line)| line.clone())
                    .collect(),
            })
        })
    }

    pub(crate) fn control(
        &self,
        id: &str,
        generation: &str,
        process: &str,
        action: &str,
    ) -> Result<HostProcessStatus> {
        self.with_activation(id, generation, |activation| {
            let entry = process_entry(activation, process)?;
            match action {
                "start" => {
                    // Start is idempotent while running/backing off. Restart is explicit.
                    if entry.intent.borrow().mode != Mode::Running
                        || matches!(
                            entry.data.lock().unwrap().status.state,
                            HostProcessState::Exited | HostProcessState::Crashed
                        )
                    {
                        entry.set_intent(Mode::Running);
                    }
                }
                "restart" => entry.set_intent(Mode::Running),
                "stop" => entry.set_intent(Mode::Stopped),
                "status" => {}
                _ => {
                    return Err(PluginError::new(
                        PLUGIN_INVALID_ACTION,
                        "unknown process action",
                    ))
                }
            }
            Ok(entry.data.lock().unwrap().status.clone())
        })
    }

    /// Explicit Core shutdown waits for group termination before returning.
    pub(crate) async fn shutdown(&self) {
        {
            let mut lifecycle = self.lifecycle.state.lock().unwrap();
            lifecycle.closed = true;
            for &group in &lifecycle.groups {
                signal_group(group, false);
            }
        }
        for activation in self.active.lock().unwrap().values() {
            for entry in activation.entries.values() {
                entry.set_intent(Mode::Revoked);
            }
        }
        let deadline = std::time::Instant::now() + STOP_GRACE;
        loop {
            if self.lifecycle.state.lock().unwrap().groups.is_empty() {
                return;
            }
            if std::time::Instant::now() >= deadline {
                break;
            }
            sleep(Duration::from_millis(10)).await;
        }
        for &group in &self.lifecycle.state.lock().unwrap().groups {
            signal_group(group, true);
        }
        // Wait for GroupGuard to finish durable removal, not merely for signal
        // delivery. Never clear records before the owning supervisors finish.
        let deadline = std::time::Instant::now() + STOP_GRACE;
        while !self.lifecycle.state.lock().unwrap().groups.is_empty() {
            if std::time::Instant::now() >= deadline {
                log::warn!("managed process shutdown timed out; retaining recovery ledger");
                break;
            }
            sleep(Duration::from_millis(10)).await;
        }
    }
    pub(crate) fn take_changed(&self) -> bool {
        self.changed.swap(false, Ordering::AcqRel)
    }
}
fn process_entry<'a>(activation: &'a Activation, process: &str) -> Result<&'a Arc<Entry>> {
    activation
        .entries
        .get(process)
        .ok_or_else(|| PluginError::new(PLUGIN_NOT_FOUND, "undeclared process"))
}
impl Drop for ProcessManager {
    fn drop(&mut self) {
        // Grant grace while a runtime is available, but retain a synchronous
        // guard: if the runtime cancels the reaper, Drop still kills every group.
        let runtime = tokio::runtime::Handle::try_current().ok();
        let mut lifecycle = self.lifecycle.state.lock().unwrap();
        lifecycle.closed = true;
        for &group in &lifecycle.groups {
            signal_group(group, runtime.is_none());
        }
        drop(lifecycle);
        for activation in self.active.lock().unwrap().values() {
            for entry in activation.entries.values() {
                entry.set_intent(Mode::Revoked);
            }
        }
        if let Some(runtime) = runtime {
            let guard = ShutdownGuard(self.lifecycle.clone());
            runtime.spawn(async move {
                sleep(STOP_GRACE).await;
                drop(guard);
            });
        }
    }
}

struct ShutdownGuard(Arc<Lifecycle>);
impl Drop for ShutdownGuard {
    fn drop(&mut self) {
        let lifecycle = self.0.state.lock().unwrap();
        for &group in &lifecycle.groups {
            signal_group(group, true);
        }
    }
}

// Guard also runs if a supervisor is cancelled while the runtime is shutting down.
struct GroupGuard {
    pid: u32,
    lifecycle: Arc<Lifecycle>,
}
impl Drop for GroupGuard {
    fn drop(&mut self) {
        let mut lifecycle = self.lifecycle.state.lock().unwrap();
        signal_group(self.pid, true);
        lifecycle.groups.remove(&self.pid);
        lifecycle.records.remove(&self.pid);
        if let Err(error) = write_ledger(&self.lifecycle.ledger, lifecycle.records.values()) {
            // A leftover record is safe: the next Core validates kernel identity.
            log::error!(
                "managed process ledger removal failed: pid={} error={error}",
                self.pid
            );
        }
    }
}
#[cfg(unix)]
fn signal_group(pid: u32, force: bool) {
    if let Ok(pid) = i32::try_from(pid) {
        // SAFETY: only a positive, registered child PGID is negated. process_group(0)
        // isolates it from Core's own process group before exec.
        unsafe {
            libc::kill(-pid, if force { libc::SIGKILL } else { libc::SIGTERM });
        }
    }
}
#[cfg(not(unix))]
fn signal_group(_pid: u32, _force: bool) {}

fn command_path(command: &str, package: Option<&Path>) -> std::io::Result<PathBuf> {
    let path = Path::new(command);
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    if command.contains('/') || command.contains('\\') {
        return package
            .map(|p| p.join(path))
            .ok_or_else(|| std::io::Error::other("package-relative command requires a package"));
    }
    // A bare command names a packaged executable when present, otherwise PATH.
    // This filesystem check runs on the supervisor, never in the broker facade.
    Ok(package
        .map(|p| p.join(path))
        .filter(|p| p.is_file())
        .unwrap_or_else(|| path.to_path_buf()))
}

async fn spawn_process(
    contribution: &ProcessContribution,
    package: Option<&Path>,
    profile_data: &Path,
    lifecycle: &Arc<Lifecycle>,
) -> std::io::Result<(Child, GroupGuard)> {
    let cwd = match contribution.cwd.as_deref() {
        Some("profileData") => {
            let path = profile_data.to_path_buf();
            tokio::task::spawn_blocking(move || std::fs::create_dir_all(path)).await??;
            Some(profile_data)
        }
        Some("package") => {
            Some(package.ok_or_else(|| std::io::Error::other("package cwd requires a package"))?)
        }
        Some(path) => Some(Path::new(path)),
        None => package,
    };
    let mut command = Command::new(command_path(&contribution.command, package)?);
    command
        .args(&contribution.args)
        .envs(&contribution.env)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    #[cfg(unix)]
    command.process_group(0);
    #[cfg(target_os = "linux")]
    {
        // SAFETY: getpid has no preconditions. The pre_exec hook calls only
        // async-signal-safe libc operations and constructs errors on failure.
        let parent = unsafe { libc::getpid() };
        unsafe {
            command.pre_exec(move || {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                // Close the fork-to-prctl race if Core died before the hook ran.
                if libc::getppid() != parent {
                    libc::_exit(127);
                }
                Ok(())
            });
        }
    }
    let mut state = lifecycle.state.lock().unwrap();
    if state.closed {
        return Err(std::io::Error::other("process manager is shutting down"));
    }
    let mut child = command.spawn()?;
    let pid = child
        .id()
        .ok_or_else(|| std::io::Error::other("spawned process has no pid"))?;
    let identity = match process_identity(pid) {
        Ok(identity) => identity,
        Err(error) => {
            // Very short-lived commands may exit before the kernel sample. They
            // still get a (non-matching) record until the normal exit path runs.
            // Unsupported platforms keep a best-effort ledger, never signal it.
            if error.kind() != std::io::ErrorKind::Unsupported && child.try_wait()?.is_none() {
                signal_group(pid, true);
                return Err(error);
            }
            ProcessIdentity {
                executable: command_path(&contribution.command, package)?,
                start_time: "unavailable/exited".into(),
            }
        }
    };
    state.records.insert(
        pid,
        LedgerEntry {
            // profile_data is constructed from the already-validated plugin ID.
            plugin_id: profile_data
                .file_name()
                .and_then(|id| id.to_str())
                .unwrap_or_default()
                .into(),
            process_id: contribution.id.clone(),
            pid,
            pgid: pid,
            identity,
        },
    );
    if let Err(error) = write_ledger(&lifecycle.ledger, state.records.values()) {
        state.records.remove(&pid);
        signal_group(pid, true);
        return Err(error); // kill_on_drop also kills/reaps the direct child.
    }
    log::info!(
        "managed process ledger registered: process={} pid={} pgid={}",
        contribution.id,
        pid,
        pid
    );
    state.groups.insert(pid);
    Ok((
        child,
        GroupGuard {
            pid,
            lifecycle: lifecycle.clone(),
        },
    ))
}

struct Readers(Vec<JoinHandle<()>>);
impl Readers {
    async fn finish(&mut self) {
        for reader in &mut self.0 {
            if timeout(Duration::from_millis(100), &mut *reader)
                .await
                .is_err()
            {
                reader.abort();
                let _ = reader.await;
            }
        }
        self.0.clear();
    }
}
impl Drop for Readers {
    fn drop(&mut self) {
        for reader in &self.0 {
            reader.abort();
        }
    }
}
async fn read_pipe(mut pipe: impl AsyncRead + Unpin, entry: Arc<Entry>, stream: HostProcessStream) {
    let mut chunk = [0u8; 4096];
    let mut line = Vec::with_capacity(LOG_LINE_BYTES);
    loop {
        match pipe.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                for &byte in &chunk[..n] {
                    if byte == b'\n' {
                        if line.last() == Some(&b'\r') {
                            line.pop();
                        }
                        entry.append(stream, &line, false);
                        line.clear();
                    } else {
                        if line.len() == LOG_LINE_BYTES {
                            entry.append(stream, &line, true);
                            line.clear();
                        }
                        line.push(byte);
                    }
                }
            }
        }
    }
    if !line.is_empty() {
        entry.append(stream, &line, false);
    }
}

async fn terminate(child: &mut Child, group: &GroupGuard, already_exited: bool) {
    #[cfg(unix)]
    {
        signal_group(group.pid, false);
        // Keep the group, not merely its leader, under supervision through grace:
        // the leader can exit first while a descendant ignores TERM.
        let grace = sleep(STOP_GRACE);
        tokio::pin!(grace);
        if !already_exited {
            tokio::select! { _ = child.wait() => {}, _ = &mut grace => {} }
        }
        grace.await;
        signal_group(group.pid, true);
    }
    #[cfg(not(unix))]
    let _ = (group, already_exited);
    let _ = child.start_kill();
    let _ = child.wait().await;
}

async fn supervise(
    entry: Arc<Entry>,
    mut receiver: watch::Receiver<Intent>,
    contribution: ProcessContribution,
    package: Option<PathBuf>,
    profile_data: PathBuf,
    lifecycle: Arc<Lifecycle>,
) {
    let mut failures = 0u32;
    let mut last_revision = u64::MAX;
    loop {
        let intent = *receiver.borrow_and_update();
        if intent.mode == Mode::Revoked {
            return;
        }
        if last_revision != intent.revision {
            failures = 0;
            last_revision = intent.revision;
        }
        if intent.mode == Mode::Stopped {
            entry.update(intent.revision, |s| {
                s.state = HostProcessState::Stopped;
                s.pid = None;
            });
            if receiver.changed().await.is_err() {
                return;
            }
            continue;
        }
        entry.update(intent.revision, |s| {
            s.state = HostProcessState::Starting;
            s.pid = None;
            s.exit_code = None;
        });
        let spawned = tokio::select! {
            biased;
            changed = receiver.changed() => { if changed.is_err() { return; } continue; }
            spawned = spawn_process(&contribution, package.as_deref(), &profile_data, &lifecycle) => spawned,
        };
        let started = Instant::now();
        let success = match spawned {
            Ok((mut child, group)) => {
                entry.update(intent.revision, |s| {
                    s.state = HostProcessState::Running;
                    s.pid = Some(group.pid as u64);
                    s.started_at = Some(now_ms());
                });
                let mut readers = Readers(Vec::new());
                if let Some(stdout) = child.stdout.take() {
                    readers.0.push(tokio::spawn(read_pipe(
                        stdout,
                        entry.clone(),
                        HostProcessStream::Stdout,
                    )));
                }
                if let Some(stderr) = child.stderr.take() {
                    readers.0.push(tokio::spawn(read_pipe(
                        stderr,
                        entry.clone(),
                        HostProcessStream::Stderr,
                    )));
                }
                let exited = tokio::select! {
                    biased;
                    _ = receiver.changed() => None,
                    result = child.wait() => Some(result),
                };
                terminate(&mut child, &group, exited.is_some()).await;
                readers.finish().await;
                drop(group);
                let Some(result) = exited else {
                    continue;
                };
                let success = result.as_ref().is_ok_and(|s| s.success());
                let code = result.ok().and_then(|s| s.code());
                entry.update(intent.revision, |s| {
                    s.pid = None;
                    s.exit_code = code;
                    s.state = if success {
                        HostProcessState::Exited
                    } else {
                        HostProcessState::Crashed
                    };
                });
                success
            }
            Err(error) => {
                entry.append(
                    HostProcessStream::Stderr,
                    error.to_string().as_bytes(),
                    false,
                );
                entry.update(intent.revision, |s| {
                    s.pid = None;
                    s.state = HostProcessState::Crashed;
                });
                false
            }
        };
        // An intent change during cleanup always wins over a policy restart.
        if receiver.borrow().revision != intent.revision {
            continue;
        }
        if started.elapsed() >= STABLE_RUN {
            failures = 0;
        }
        let restart = match contribution.restart {
            ProcessRestart::Never => false,
            ProcessRestart::OnFailure => !success,
            ProcessRestart::Always => true,
        };
        if !restart || failures >= MAX_RESTARTS {
            if restart {
                entry.update(intent.revision, |s| s.state = HostProcessState::Crashed);
            }
            if receiver.changed().await.is_err() {
                return;
            }
            continue;
        }
        let delay = BACKOFF_BASE * (1 << failures);
        failures += 1;
        entry.update(intent.revision, |s| s.state = HostProcessState::Backoff);
        tokio::select! {
            biased;
            changed = receiver.changed() => { if changed.is_err() { return; } }
            _ = sleep(delay) => {
                entry.update(intent.revision, |s| s.restarts = s.restarts.saturating_add(1).min(MAX_SAFE_INTEGER));
            }
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;

    static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);
    struct Directory(PathBuf);
    impl Directory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "se-managed-process-{}-{}-{}",
                std::process::id(),
                now_ms(),
                NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    // Shells are explicit test payloads only. Production never interprets args.
    fn script(text: &str) -> ProcessContribution {
        ProcessContribution {
            id: "worker".into(),
            command: "/bin/sh".into(),
            args: vec!["-c".into(), text.into()],
            env: BTreeMap::new(),
            cwd: None,
            autostart: true,
            restart: ProcessRestart::Never,
        }
    }
    async fn wait_for(mut condition: impl FnMut() -> bool) {
        timeout(Duration::from_secs(10), async {
            while !condition() {
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("process condition timed out");
    }
    fn status(manager: &ProcessManager) -> HostProcessStatus {
        manager.list("test.plugin", "g1").unwrap()["worker"].clone()
    }
    fn running(pid: u32) -> bool {
        // kill(0) also reports zombies, which cannot execute or retain children.
        // macOS has no /proc; ps works on both test platforms.
        let output = std::process::Command::new("/bin/ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()
            .unwrap();
        let state = String::from_utf8_lossy(&output.stdout);
        !state.trim().is_empty() && !state.trim_start().starts_with('Z')
    }

    #[test]
    fn ledger_round_trip_and_atomic_replacement() {
        let directory = Directory::new();
        let path = ledger_path(&directory.0);
        let entry = LedgerEntry {
            plugin_id: "test.plugin".into(),
            process_id: "worker".into(),
            pid: 123,
            pgid: 123,
            identity: ProcessIdentity {
                executable: PathBuf::from("/bin/sleep"),
                start_time: "kernel-start-identity".into(),
            },
        };
        assert!(read_ledger(&path).unwrap().is_empty());
        write_ledger(&path, std::iter::once(&entry)).unwrap();
        assert_eq!(read_ledger(&path).unwrap(), vec![entry]);
        assert!(!path.with_extension("json.tmp").exists());
        write_ledger(&path, std::iter::empty()).unwrap();
        assert!(read_ledger(&path).unwrap().is_empty());
    }

    // Exercise the production spawn/ledger path without a supervisor task. The
    // intentionally forgotten guard simulates losing all in-memory Core cleanup;
    // retain Child's kill_on_drop and wait handle so assertion failures cannot
    // leave a sleep process behind in the test runner.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    async fn orphan_sleep(profile: &Path) -> Child {
        let lifecycle = Arc::new(Lifecycle {
            state: Mutex::new(LifecycleState::default()),
            ledger: ledger_path(profile),
        });
        let mut process = script("");
        process.command = "/bin/sleep".into();
        process.args = vec!["60".into()];
        let data = profile.join("plugins/data/test.plugin");
        let (child, guard) = spawn_process(&process, None, &data, &lifecycle)
            .await
            .unwrap();
        std::mem::forget(guard);
        child
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[tokio::test]
    async fn startup_reap_does_not_signal_mismatched_identity() {
        let directory = Directory::new();
        let mut child = orphan_sleep(&directory.0).await;
        let path = ledger_path(&directory.0);
        let entry = read_ledger(&path).unwrap().remove(0);
        assert!(identity_matches(entry.pid, entry.pgid, &entry.identity));
        // Independently verify BOTH parts of identity, not just an impossible PID.
        for wrong_executable in [false, true] {
            let mut stale = entry.clone();
            if wrong_executable {
                stale.identity.executable = PathBuf::from("/definitely/not/the/executable");
            } else {
                stale.identity.start_time.push_str("-reused-pid");
            }
            write_ledger(&path, std::iter::once(&stale)).unwrap();
            reap_orphans(&directory.0).await.unwrap();
            assert!(
                child.try_wait().unwrap().is_none(),
                "mismatch must not signal sleep"
            );
            assert!(read_ledger(&path).unwrap().is_empty());
        }
        child.kill().await.unwrap();
        child.wait().await.unwrap();
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[tokio::test]
    async fn fresh_host_reaps_recorded_sleep_after_core_death() {
        let directory = Directory::new();
        let mut child = orphan_sleep(&directory.0).await;
        let path = ledger_path(&directory.0);
        let entries = read_ledger(&path).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].plugin_id, "test.plugin");
        assert_eq!(entries[0].process_id, "worker");
        assert_eq!(entries[0].pid, child.id().unwrap());
        assert!(identity_matches(
            entries[0].pid,
            entries[0].pgid,
            &entries[0].identity
        ));
        let host = crate::PluginHost::open(directory.0.clone(), crate::HostOptions::default())
            .await
            .unwrap();
        let exit = timeout(Duration::from_secs(3), child.wait())
            .await
            .unwrap()
            .unwrap();
        assert!(!exit.success());
        assert!(read_ledger(&path).unwrap().is_empty());
        host.shutdown().await.unwrap();
        assert!(read_ledger(&path).unwrap().is_empty());
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[tokio::test]
    async fn startup_reap_escalates_when_group_leader_exits_first() {
        use tokio::io::{AsyncBufReadExt, BufReader};
        struct Cleanup(u32);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                signal_group(self.0, true);
            }
        }
        let directory = Directory::new();
        let lifecycle = Arc::new(Lifecycle {
            state: Mutex::new(LifecycleState::default()),
            ledger: ledger_path(&directory.0),
        });
        let mut process = script(
            "trap 'exit 0' TERM; /bin/bash -c 'trap \"\" TERM; echo $$; while :; do :; done' & wait",
        );
        // macOS /bin/sh execs /bin/bash, intentionally changing executable
        // identity. Use the final executable to exercise escalation, not skip.
        process.command = "/bin/bash".into();
        let (mut child, guard) = spawn_process(
            &process,
            None,
            &directory.0.join("plugins/data/test.plugin"),
            &lifecycle,
        )
        .await
        .unwrap();
        let _cleanup = Cleanup(child.id().unwrap());
        std::mem::forget(guard);
        let mut stdout = BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        timeout(Duration::from_secs(3), stdout.read_line(&mut line))
            .await
            .unwrap()
            .unwrap();
        let descendant: u32 = line.trim().parse().unwrap();
        assert_ne!(descendant, child.id().unwrap());
        assert!(running(descendant));
        let entry = read_ledger(&ledger_path(&directory.0)).unwrap().remove(0);
        assert_eq!(process_identity(entry.pid).unwrap(), entry.identity);
        reap_orphans(&directory.0).await.unwrap();
        timeout(Duration::from_secs(3), child.wait())
            .await
            .unwrap()
            .unwrap();
        wait_for(|| !running(descendant)).await;
        assert!(read_ledger(&ledger_path(&directory.0)).unwrap().is_empty());
    }

    #[tokio::test]
    async fn graceful_shutdown_empties_ledger() {
        let directory = Directory::new();
        let manager = ProcessManager::new(directory.0.clone());
        let mut process = script("");
        process.command = "/bin/sleep".into();
        process.args = vec!["60".into()];
        manager
            .activate("test.plugin", "g1", None, &[process])
            .unwrap();
        wait_for(|| status(&manager).pid.is_some()).await;
        let pid = status(&manager).pid.unwrap() as u32;
        assert_eq!(read_ledger(&ledger_path(&directory.0)).unwrap().len(), 1);
        manager.shutdown().await;
        assert!(manager.lifecycle.state.lock().unwrap().groups.is_empty());
        assert!(read_ledger(&ledger_path(&directory.0)).unwrap().is_empty());
        wait_for(|| !running(pid)).await;
    }

    #[tokio::test]
    async fn process_restarts_with_backoff_and_stops_on_disable() {
        let directory = Directory::new();
        let manager = ProcessManager::new(directory.0.clone());
        let mut process = script("exit 7");
        process.restart = ProcessRestart::OnFailure;
        manager
            .activate("test.plugin", "g1", None, &[process])
            .unwrap();
        wait_for(|| status(&manager).state == HostProcessState::Backoff).await;
        let first = Instant::now();
        wait_for(|| status(&manager).restarts >= 1).await;
        assert!(first.elapsed() >= Duration::from_millis(70));
        wait_for(|| {
            let s = status(&manager);
            s.state == HostProcessState::Backoff && s.restarts == 1
        })
        .await;
        let second = Instant::now();
        wait_for(|| status(&manager).restarts >= 2).await;
        assert!(second.elapsed() >= Duration::from_millis(170));
        wait_for(|| status(&manager).state == HostProcessState::Backoff).await;
        let entry = manager.active.lock().unwrap()["test.plugin"].entries["worker"].clone();
        let restarts = entry.data.lock().unwrap().status.restarts;
        manager.revoke("test.plugin", "g1");
        assert_eq!(
            manager.list("test.plugin", "g1").unwrap_err().code,
            PLUGIN_DISABLED
        );
        sleep(Duration::from_millis(900)).await;
        assert_eq!(entry.data.lock().unwrap().status.restarts, restarts);
        assert!(entry.intent.borrow().mode == Mode::Revoked);
    }

    #[tokio::test]
    async fn process_group_is_killed_on_core_shutdown() {
        let directory = Directory::new();
        let manager = ProcessManager::new(directory.0.clone());
        let process = script(
            "trap '' TERM; /bin/sh -c 'trap \"\" TERM; while :; do sleep 1; done' & echo $!; wait",
        );
        manager
            .activate("test.plugin", "g1", None, &[process])
            .unwrap();
        wait_for(|| {
            !manager
                .logs("test.plugin", "g1", "worker", 10)
                .unwrap()
                .lines
                .is_empty()
        })
        .await;
        let leader = status(&manager).pid.unwrap() as u32;
        let descendant: u32 = manager
            .logs("test.plugin", "g1", "worker", 10)
            .unwrap()
            .lines[0]
            .text
            .parse()
            .unwrap();
        assert!(running(leader));
        assert!(running(descendant));
        drop(manager);
        wait_for(|| !running(leader) && !running(descendant)).await;
    }

    #[tokio::test]
    async fn process_logs_are_bounded() {
        let directory = Directory::new();
        let manager = ProcessManager::new(directory.0.clone());
        // Exceed both rings and send one oversized newline-free record.
        let process = script("i=0; while [ $i -lt 1200 ]; do printf 'out-%s-abcdefghijklmnopqrstuvwxyz0123456789\\n' \"$i\"; printf 'err-%s-abcdefghijklmnopqrstuvwxyz0123456789\\n' \"$i\" >&2; i=$((i+1)); done; i=0; while [ $i -lt 9000 ]; do printf x; i=$((i+1)); done; printf '\\n'");
        manager
            .activate("test.plugin", "g1", None, &[process])
            .unwrap();
        wait_for(|| status(&manager).state == HostProcessState::Exited).await;
        let logs = manager
            .logs("test.plugin", "g1", "worker", MAX_PROCESS_LOG_TAIL)
            .unwrap();
        assert!(logs.truncated);
        assert!(logs.lines.len() <= 2 * LOG_LINES_PER_STREAM);
        assert!(logs
            .lines
            .iter()
            .all(|line| line.text.len() <= LOG_LINE_BYTES));
        assert!(
            logs.lines.iter().map(|line| line.text.len()).sum::<usize>()
                <= 2 * LOG_BYTES_PER_STREAM
        );
        assert!(logs
            .lines
            .iter()
            .any(|line| line.stream == HostProcessStream::Stdout));
        assert!(logs
            .lines
            .iter()
            .any(|line| line.stream == HostProcessStream::Stderr));
        validate_host_process_logs_reply(&logs).unwrap();
        let tail = manager.logs("test.plugin", "g1", "worker", 3).unwrap();
        assert_eq!(tail.lines.len(), 3);
        assert_eq!(tail.lines, logs.lines[logs.lines.len() - 3..]);
        assert!(manager
            .logs("test.plugin", "g1", "worker", MAX_PROCESS_LOG_TAIL + 1)
            .is_err());
        assert!(manager
            .logs("test.plugin", "g1", "worker", 0)
            .unwrap()
            .lines
            .is_empty());
    }

    #[tokio::test]
    async fn process_generations_controls_and_spawn_options() {
        use std::os::unix::fs::PermissionsExt;
        let directory = Directory::new();
        let executable = directory.0.join("worker");
        std::fs::write(
            &executable,
            "#!/bin/sh\nprintf '%s\\n%s\\n%s\\n' \"$PWD\" \"$VALUE\" \"$1\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        let manager = ProcessManager::new(directory.0.clone());
        let mut contribution = script("");
        contribution.command = "worker".into();
        contribution.args = vec!["literal ; $(not-a-command)".into()];
        contribution
            .env
            .insert("VALUE".into(), "value with spaces".into());
        contribution.cwd = Some("profileData".into());
        contribution.autostart = false;
        manager
            .activate(
                "test.plugin",
                "g1",
                Some(directory.0.clone()),
                &[contribution.clone()],
            )
            .unwrap();
        assert_eq!(status(&manager).state, HostProcessState::Stopped);
        assert!(manager.take_changed());
        assert!(!manager.take_changed());
        assert_eq!(
            manager
                .control("test.plugin", "stale", "worker", "start")
                .unwrap_err()
                .code,
            PLUGIN_STALE_GENERATION
        );
        assert_eq!(
            manager
                .control("test.plugin", "g1", "missing", "start")
                .unwrap_err()
                .code,
            PLUGIN_NOT_FOUND
        );
        manager
            .control("test.plugin", "g1", "worker", "start")
            .unwrap();
        wait_for(|| status(&manager).state == HostProcessState::Exited).await;
        let logs = manager.logs("test.plugin", "g1", "worker", 10).unwrap();
        assert!(logs.lines[0].text.ends_with("plugins/data/test.plugin"));
        assert_eq!(logs.lines[1].text, "value with spaces");
        assert_eq!(logs.lines[2].text, "literal ; $(not-a-command)");
        manager
            .activate(
                "test.plugin",
                "g2",
                Some(directory.0.clone()),
                &[contribution],
            )
            .unwrap();
        manager.revoke("test.plugin", "g1");
        assert!(manager.list("test.plugin", "g2").is_ok());
        assert_eq!(
            manager.list("test.plugin", "g1").unwrap_err().code,
            PLUGIN_STALE_GENERATION
        );
    }

    #[tokio::test]
    async fn process_stop_escalates_and_restart_cap_is_finite() {
        let directory = Directory::new();
        let manager = ProcessManager::new(directory.0.clone());
        let process = script("trap '' TERM; echo ready; while :; do sleep 1; done");
        manager
            .activate("test.plugin", "g1", None, &[process])
            .unwrap();
        wait_for(|| {
            !manager
                .logs("test.plugin", "g1", "worker", 1)
                .unwrap()
                .lines
                .is_empty()
        })
        .await;
        let pid = status(&manager).pid.unwrap() as u32;
        manager
            .control("test.plugin", "g1", "worker", "stop")
            .unwrap();
        wait_for(|| status(&manager).pid.is_none()).await;
        assert!(!running(pid));
        let mut missing = script("");
        missing.command = "/no/such/se-plugin-executable".into();
        missing.restart = ProcessRestart::Always;
        manager
            .activate("test.plugin", "g2", None, &[missing])
            .unwrap();
        wait_for(|| {
            let s = &manager.list("test.plugin", "g2").unwrap()["worker"];
            s.state == HostProcessState::Crashed && s.restarts == MAX_RESTARTS as u64
        })
        .await;
        sleep(Duration::from_millis(200)).await;
        assert_eq!(
            manager.list("test.plugin", "g2").unwrap()["worker"].restarts,
            MAX_RESTARTS as u64
        );
    }
}
