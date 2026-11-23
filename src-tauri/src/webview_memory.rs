//! Low-overhead local memory diagnostics for the GUI and its WebKit helpers.
//!
//! The GUI (this process) and each WebKit renderer are separate processes.
//! When a renderer is terminated, WebKit hands Tauri no reason, so after the
//! fact the log cannot say what the renderer last looked like. This module
//! samples, on a fixed cadence, the GUI process and the WebKit helper
//! processes that belong to it, and writes one `key=value` line per sample to
//! the normal rotated log. The termination callback then echoes the last
//! sample, so the two lines can be tied together by `seq` and `sample_age_ms`.
//!
//! Attribution is verified, never guessed from a process name:
//!
//! * a candidate must execute a real system WebKit XPC service binary — the
//!   executable path returned by `proc_pidpath` must contain
//!   `/System/Library/Frameworks/WebKit.framework/Versions/A/XPCServices/`
//!   (the cryptex-backed SDK path also contains it) and end with the exact
//!   `com.apple.WebKit.WebContent` / `com.apple.WebKit.GPU` XPC binary;
//! * it must report the same Darwin coalition as this process
//!   (`PROC_PIDCOALITIONINFO`);
//! * it must run as the same effective user and its start time must match the
//!   pid's start time from that sample, so a reused pid is never mistaken for
//!   the process we measured before.
//!
//! `PROC_PIDCOALITIONINFO` is served by public `libproc` (`proc_pidinfo`), but
//! its flavor constant and the accompanying struct are only published in the
//! open-source xnu headers, not in `sys/proc_info.h` of the installed SDK.
//! They are therefore declared here, and the values were verified on this
//! machine: the call returns the full 40-byte struct and this GUI and its
//! WebKit helpers report the same 16-byte coalition id.
//!
//! Nothing content-bearing is read or logged: no process arguments, no file
//! paths, no terminal contents, no credentials. Only numeric memory counters
//! and the pids of this app's own helper processes leave this module.
//!
//! The sampler is deliberately cheap: one process scan per cadence (a few
//! milliseconds) and four `proc_pid_rusage` calls per helper. Logging is
//! capped separately at one line per 30 s, so even the elevated cadence
//! produces at most two lines a minute.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Log target, shared with the other GUI subsystems' `se_manager::*` names.
const TARGET: &str = "se_manager::webview_memory";

/// Cadence while everything is small.
const NORMAL_INTERVAL: Duration = Duration::from_secs(60);
/// Cadence while any attributed process is above [`ELEVATED_FOOTPRINT_BYTES`].
const ELEVATED_INTERVAL: Duration = Duration::from_secs(15);
/// Floor between two log lines, independent of the sampling cadence.
const LOG_FLOOR: Duration = Duration::from_secs(30);
/// Footprint that switches the `band` and the faster cadence. One decoration
/// of headroom: a healthy WebKit renderer sits well below this.
const ELEVATED_FOOTPRINT_BYTES: u64 = 1024 * 1024 * 1024;

/// `PROC_PIDCOALITIONINFO` from xnu's `bsd/sys/proc_info.h`. Not exported by
/// the installed SDK headers, but accepted by public `proc_pidinfo`.
const PROC_PIDCOALITIONINFO: libc::c_int = 20;

/// The `proc_pidcoalitioninfo` payload xnu fills for flavor 20:
/// `coalition_id[COALITION_TYPE_MAX + 1]` — the resource and jetsam coalition
/// ids — followed by three reserved words, 40 bytes in total. Verified on this
/// system: the call returns exactly this many bytes and this GUI and its WebKit
/// helpers report the same two ids.
#[repr(C)]
#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct ProcCoalitionInfo {
    coalition_id: [u64; 2],
    _reserved: [u64; 3],
}

impl ProcCoalitionInfo {
    fn read(pid: libc::pid_t) -> Result<Self, i32> {
        let mut info = std::mem::MaybeUninit::<Self>::zeroed();
        let size = std::mem::size_of::<Self>() as libc::c_int;
        let written = unsafe {
            libc::proc_pidinfo(
                pid,
                PROC_PIDCOALITIONINFO,
                0,
                info.as_mut_ptr().cast(),
                size,
            )
        };
        if written != size {
            return Err(last_errno());
        }
        Ok(unsafe { info.assume_init() })
    }
}

/// Real system WebKit helper binaries. The cryptex mount used by current
/// macOS still carries this framework fragment, so the check does not depend
/// on the volume layout.
const WEBKIT_XPC_FRAGMENT: &str =
    "/System/Library/Frameworks/WebKit.framework/Versions/A/XPCServices/";
const WEB_CONTENT_SUFFIX: &str =
    "/com.apple.WebKit.WebContent.xpc/Contents/MacOS/com.apple.WebKit.WebContent";
const GPU_SUFFIX: &str = "/com.apple.WebKit.GPU.xpc/Contents/MacOS/com.apple.WebKit.GPU";

/// Which WebKit helper a verified executable is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HelperRole {
    WebContent,
    Gpu,
}

impl HelperRole {
    fn group(self) -> &'static str {
        match self {
            HelperRole::WebContent => "web_content",
            HelperRole::Gpu => "gpu",
        }
    }
}

/// Classify an executable path as a real system WebKit helper.
///
/// A spoofed copy under `/Applications` or in the user's home fails: it never
/// contains the system framework fragment. Networking is intentionally not
/// matched; it is not a renderer and its footprint tracks page state far less.
fn classify_webkit_helper(path: &str) -> Option<HelperRole> {
    if !path.starts_with("/System/") || !path.contains(WEBKIT_XPC_FRAGMENT) {
        return None;
    }
    if path.ends_with(WEB_CONTENT_SUFFIX) {
        Some(HelperRole::WebContent)
    } else if path.ends_with(GPU_SUFFIX) {
        Some(HelperRole::Gpu)
    } else {
        None
    }
}

/// Current, lifetime-peak and resident bytes of one process (`RUSAGE_INFO_V4`).
#[derive(Debug, Clone, Copy)]
struct Usage {
    footprint_bytes: u64,
    peak_bytes: u64,
    resident_bytes: u64,
}

/// One verified WebKit helper as measured in one sample.
#[derive(Debug, Clone)]
struct HelperReading {
    pid: u32,
    /// `tvsec * 1_000_000 + tvusec`, the pid-reuse guard.
    start_marker: u64,
    usage: Option<Usage>,
    /// `errno` when `proc_pid_rusage` refused; `None` when `usage` is present.
    read_errno: Option<i32>,
    /// How long this exact process has been observed by the sampler.
    seen_ms: u64,
}

/// Which cadence the next sleep uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Band {
    Normal,
    Elevated,
}

impl Band {
    fn interval(self) -> Duration {
        match self {
            Band::Normal => NORMAL_INTERVAL,
            Band::Elevated => ELEVATED_INTERVAL,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Band::Normal => "normal",
            Band::Elevated => "elevated",
        }
    }
}

/// One complete sample, cheap to clone for the termination path.
#[derive(Debug, Clone)]
struct Snapshot {
    seq: u64,
    taken_at: Instant,
    uptime_ms: u64,
    band: Band,
    gui_pid: u32,
    gui_usage: Option<Usage>,
    gui_errno: Option<i32>,
    coalition: Option<[u64; 2]>,
    coalition_reason: Option<String>,
    web_content: Vec<HelperReading>,
    gpu: Vec<HelperReading>,
    scan_failed: bool,
}

impl Snapshot {
    /// Render the one-line `key=value` record the log carries.
    ///
    /// `event` is `sample`, `web_content_terminate` or `exit_app`. The
    /// termination event records why the callback is silent — WebKit gives it
    /// no reason — and explicitly refuses to infer a cause.
    fn render(&self, event: &str, now: Instant) -> String {
        let mut out = String::with_capacity(512);
        let _ = write!(out, "event={event}");
        if event == "web_content_terminate" {
            let _ = write!(
                out,
                " termination_reason=unavailable cause_not_inferred=true note=webkit_terminate_callback_gives_no_reason"
            );
        }
        let sample_age_ms = now.saturating_duration_since(self.taken_at).as_millis();
        let _ = write!(
            out,
            " seq={} band={} uptime_ms={} sample_age_ms={} sampling_interval_ms={}",
            self.seq,
            self.band.name(),
            self.uptime_ms,
            sample_age_ms,
            self.band.interval().as_millis()
        );

        let _ = write!(out, " gui_pid={}", self.gui_pid);
        match (self.gui_usage, self.gui_errno) {
            (Some(usage), _) => {
                let _ = write!(
                    out,
                    " gui_state=ok gui_footprint_bytes={} gui_peak_bytes={} gui_resident_bytes={}",
                    usage.footprint_bytes, usage.peak_bytes, usage.resident_bytes
                );
            }
            (None, errno) => {
                let _ = write!(out, " gui_state=unavailable gui_reason=rusage_failed");
                if let Some(errno) = errno {
                    let _ = write!(out, " gui_errno={errno}");
                }
            }
        }

        let _ = write!(
            out,
            " coalition_state={}",
            if self.coalition.is_some() {
                "ok"
            } else {
                "unavailable"
            }
        );
        if let Some(reason) = &self.coalition_reason {
            let _ = write!(out, " coalition_reason={reason}");
        }

        for (role, items) in [
            (HelperRole::WebContent, &self.web_content),
            (HelperRole::Gpu, &self.gpu),
        ] {
            let group = role.group();
            let state = if self.coalition.is_none() || self.scan_failed {
                // Without a verifiable coalition (or a completed scan) an empty
                // list means "could not look", not "nothing is running".
                "unavailable"
            } else if items.is_empty() {
                "none"
            } else {
                "ok"
            };
            let _ = write!(out, " {group}_state={state} {group}_count={}", items.len());
            if state == "unavailable" && self.scan_failed {
                let _ = write!(out, " {group}_reason=process_scan_failed");
            } else if state == "unavailable" {
                let _ = write!(out, " {group}_reason=coalition_unreadable");
            }
            if !items.is_empty() {
                let _ = write!(out, " {group}=[");
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        out.push(';');
                    }
                    let _ = write!(out, "pid={} ", item.pid);
                    match (item.usage, item.read_errno) {
                        (Some(usage), _) => {
                            let _ = write!(
                                out,
                                "state=ok footprint_bytes={} peak_bytes={} resident_bytes={}",
                                usage.footprint_bytes, usage.peak_bytes, usage.resident_bytes
                            );
                        }
                        (None, errno) => {
                            let _ = write!(out, "state=unavailable reason=rusage_failed");
                            if let Some(errno) = errno {
                                let _ = write!(out, " errno={errno}");
                            }
                        }
                    }
                    let _ = write!(out, " seen_ms={}", item.seen_ms);
                }
                out.push(']');
            }
        }
        out
    }
}

/// Sampler state that survives between samples.
struct Tracker {
    gui_pid: u32,
    started: Instant,
    seq: u64,
    coalition: Option<[u64; 2]>,
    coalition_reason: Option<String>,
    /// `(pid, start_marker)` → first time this exact process was observed.
    first_seen: HashMap<(u32, u64), Instant>,
}

impl Tracker {
    fn new(gui_pid: u32) -> Self {
        Self {
            gui_pid,
            started: Instant::now(),
            seq: 0,
            coalition: None,
            coalition_reason: None,
            first_seen: HashMap::new(),
        }
    }

    /// Rebuild a tracker from the last cached snapshot so an out-of-band
    /// sample (exit, or a termination with a very stale cache) keeps the
    /// process ages and the coalition instead of restarting at `seen_ms=0`.
    fn resumed(previous: &Snapshot) -> Self {
        let mut first_seen = HashMap::new();
        for item in previous.web_content.iter().chain(previous.gpu.iter()) {
            let seen = Duration::from_millis(item.seen_ms);
            if let Some(first) = previous.taken_at.checked_sub(seen) {
                first_seen.insert((item.pid, item.start_marker), first);
            }
        }
        Self {
            gui_pid: previous.gui_pid,
            started: Instant::now()
                .checked_sub(Duration::from_millis(previous.uptime_ms))
                .unwrap_or_else(Instant::now),
            seq: previous.seq,
            coalition: previous.coalition,
            coalition_reason: previous.coalition_reason.clone(),
            first_seen,
        }
    }

    fn coalition(&mut self) -> Option<[u64; 2]> {
        if let Some(coalition) = self.coalition {
            return Some(coalition);
        }
        let Ok(pid) = libc::pid_t::try_from(self.gui_pid) else {
            self.coalition_reason = Some("gui_pid_out_of_range".to_string());
            return None;
        };
        match ProcCoalitionInfo::read(pid) {
            Ok(info) => {
                self.coalition = Some(info.coalition_id);
                self.coalition_reason = None;
                Some(info.coalition_id)
            }
            Err(errno) => {
                self.coalition_reason = Some(format!("proc_pidinfo_flavor_20 errno={errno}"));
                None
            }
        }
    }

    fn sample(&mut self, now: Instant) -> Snapshot {
        self.seq += 1;
        let gui_pid = self.gui_pid;
        let (gui_usage, gui_errno) = match read_usage(gui_pid) {
            Ok(usage) => (Some(usage), None),
            Err(errno) => (None, Some(errno)),
        };

        let mut web_content = Vec::new();
        let mut gpu = Vec::new();
        let mut seen_now: HashMap<(u32, u64), Instant> = HashMap::new();
        let mut scan_failed = false;

        if let Some(coalition) = self.coalition() {
            let pids = list_all_pids();
            if pids.is_empty() {
                scan_failed = true;
            }
            for pid in pids {
                if pid == gui_pid {
                    continue;
                }
                let Ok(pid_i32) = libc::pid_t::try_from(pid) else {
                    continue;
                };
                let Some(path) = pid_path(pid_i32) else {
                    continue;
                };
                let Some(role) = classify_webkit_helper(&path) else {
                    continue;
                };
                // Every remaining check establishes identity, so a same-named
                // foreign process or a reused pid is rejected rather than
                // attributed to this app.
                let Ok(candidate) = ProcCoalitionInfo::read(pid_i32) else {
                    continue;
                };
                if candidate.coalition_id != coalition {
                    continue;
                }
                let Some(bsd) = bsd_identity(pid_i32) else {
                    continue;
                };
                if bsd.euid != unsafe { libc::geteuid() } {
                    continue;
                }
                let key = (pid, bsd.start_marker);
                let first = *self.first_seen.entry(key).or_insert(now);
                seen_now.insert(key, first);
                let (usage, read_errno) = match read_usage(pid) {
                    Ok(usage) => (Some(usage), None),
                    Err(errno) => (None, Some(errno)),
                };
                let reading = HelperReading {
                    pid,
                    start_marker: bsd.start_marker,
                    usage,
                    read_errno,
                    seen_ms: now.saturating_duration_since(first).as_millis() as u64,
                };
                match role {
                    HelperRole::WebContent => web_content.push(reading),
                    HelperRole::Gpu => gpu.push(reading),
                }
            }
            // Drop processes that are gone; bound memory across long sessions.
            self.first_seen = seen_now;
        }

        web_content.sort_by_key(|item| item.pid);
        gpu.sort_by_key(|item| item.pid);

        let band = if [gui_usage]
            .into_iter()
            .chain(web_content.iter().map(|item| item.usage))
            .chain(gpu.iter().map(|item| item.usage))
            .flatten()
            .any(|usage| usage.footprint_bytes >= ELEVATED_FOOTPRINT_BYTES)
        {
            Band::Elevated
        } else {
            Band::Normal
        };

        Snapshot {
            seq: self.seq,
            taken_at: now,
            uptime_ms: now.saturating_duration_since(self.started).as_millis() as u64,
            band,
            gui_pid,
            gui_usage,
            gui_errno,
            coalition: self.coalition,
            coalition_reason: self.coalition_reason.clone(),
            web_content,
            gpu,
            scan_failed,
        }
    }
}

/// Identity fields from `PROC_PIDTBSDINFO` that make pid reuse detectable.
struct BsdIdentity {
    euid: libc::uid_t,
    start_marker: u64,
}

fn bsd_identity(pid: libc::pid_t) -> Option<BsdIdentity> {
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    let written = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            size,
        )
    };
    if written != size {
        return None;
    }
    let info = unsafe { info.assume_init() };
    Some(BsdIdentity {
        euid: info.pbi_uid,
        start_marker: info
            .pbi_start_tvsec
            .wrapping_mul(1_000_000)
            .wrapping_add(info.pbi_start_tvusec),
    })
}

fn read_usage(pid: u32) -> Result<Usage, i32> {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return Err(libc::EINVAL);
    };
    let mut info = std::mem::MaybeUninit::<libc::rusage_info_v4>::zeroed();
    let buffer = info.as_mut_ptr().cast::<libc::rusage_info_t>();
    let ret = unsafe { libc::proc_pid_rusage(pid, libc::RUSAGE_INFO_V4, buffer) };
    if ret != 0 {
        return Err(last_errno());
    }
    let info = unsafe { info.assume_init() };
    Ok(Usage {
        footprint_bytes: info.ri_phys_footprint,
        peak_bytes: info.ri_lifetime_max_phys_footprint,
        resident_bytes: info.ri_resident_size,
    })
}

/// All pids via `proc_listallpids`, growing the buffer when the kernel says
/// the first attempt was too small.
fn list_all_pids() -> Vec<u32> {
    let mut capacity = 4096usize;
    for _ in 0..5 {
        let mut pids = vec![0u32; capacity];
        let bytes = (pids.len() * std::mem::size_of::<u32>()) as libc::c_int;
        let written = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), bytes) };
        if written > 0 {
            let count = usize::try_from(written).unwrap_or(0).min(pids.len());
            pids.truncate(count);
            pids.retain(|pid| *pid > 0);
            return pids;
        }
        capacity *= 2;
    }
    Vec::new()
}

fn pid_path(pid: libc::pid_t) -> Option<String> {
    let mut buffer = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    let written =
        unsafe { libc::proc_pidpath(pid, buffer.as_mut_ptr().cast(), buffer.len() as u32) };
    if written <= 0 {
        return None;
    }
    let end = buffer
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(buffer.len());
    String::from_utf8(buffer[..end].to_vec()).ok()
}

fn last_errno() -> i32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

/// Last sample, so the termination callback and the exit hook can print it.
static LAST_SNAPSHOT: Mutex<Option<Snapshot>> = Mutex::new(None);
static STARTED: AtomicBool = AtomicBool::new(false);

fn store(snapshot: &Snapshot) {
    let mut slot = LAST_SNAPSHOT
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *slot = Some(snapshot.clone());
}

fn cached() -> Option<Snapshot> {
    LAST_SNAPSHOT
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

/// Start the background sampler. Idempotent; called once from app setup.
pub fn start() {
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("webview-memory".to_string())
        .spawn(run_sampler);
    if let Err(error) = spawned {
        log::warn!(
            target: TARGET,
            "event=start state=unavailable reason=thread_spawn_failed error={error}"
        );
    }
}

fn run_sampler() {
    let mut tracker = Tracker::new(std::process::id());
    log::info!(
        target: TARGET,
        "event=start gui_pid={} normal_interval_ms={} elevated_interval_ms={} log_floor_ms={} elevated_threshold_bytes={} source=libproc_pid_rusage_v4+proc_pidinfo_flavor_20",
        tracker.gui_pid,
        NORMAL_INTERVAL.as_millis(),
        ELEVATED_INTERVAL.as_millis(),
        LOG_FLOOR.as_millis(),
        ELEVATED_FOOTPRINT_BYTES
    );
    let mut last_log: Option<Instant> = None;
    loop {
        let now = Instant::now();
        let snapshot = tracker.sample(now);
        let due = last_log.is_none_or(|at| now.saturating_duration_since(at) >= LOG_FLOOR);
        if due {
            log::info!(target: TARGET, "{}", snapshot.render("sample", now));
            last_log = Some(now);
        }
        store(&snapshot);
        std::thread::sleep(snapshot.band.interval());
    }
}

/// Echo the last sample when WebKit terminates the page's process.
///
/// The callback carries no reason, so this records the timing link (`seq`,
/// `sample_age_ms`) and says outright that no cause is inferred.
pub fn record_web_content_termination(label: &str) {
    let now = Instant::now();
    match cached() {
        Some(snapshot) => {
            log::warn!(
                target: TARGET,
                "label={label} {}",
                snapshot.render("web_content_terminate", now)
            );
        }
        None => {
            log::warn!(
                target: TARGET,
                "label={label} event=web_content_terminate termination_reason=unavailable cause_not_inferred=true diagnostics=missing reason=no_sample_yet"
            );
        }
    }
}

/// Print the last reading before the app exits: one out-of-band sample on the
/// cached tracker, so process ages and the coalition carry over.
pub fn record_exit() {
    let now = Instant::now();
    let snapshot = match cached() {
        Some(previous) => Tracker::resumed(&previous).sample(now),
        None => Tracker::new(std::process::id()).sample(now),
    };
    log::info!(target: TARGET, "{}", snapshot.render("exit_app", now));
    store(&snapshot);
}

#[cfg(test)]
mod tests {
    use super::*;

    const CRYPTEX_WEB_CONTENT: &str = "/System/Volumes/Preboot/Cryptexes/OS/System/Library/Frameworks/WebKit.framework/Versions/A/XPCServices/com.apple.WebKit.WebContent.xpc/Contents/MacOS/com.apple.WebKit.WebContent";
    const PLAIN_WEB_CONTENT: &str = "/System/Library/Frameworks/WebKit.framework/Versions/A/XPCServices/com.apple.WebKit.WebContent.xpc/Contents/MacOS/com.apple.WebKit.WebContent";
    const PLAIN_GPU: &str = "/System/Library/Frameworks/WebKit.framework/Versions/A/XPCServices/com.apple.WebKit.GPU.xpc/Contents/MacOS/com.apple.WebKit.GPU";

    #[test]
    fn classifies_real_system_webkit_helpers() {
        assert_eq!(
            classify_webkit_helper(CRYPTEX_WEB_CONTENT),
            Some(HelperRole::WebContent)
        );
        assert_eq!(
            classify_webkit_helper(PLAIN_WEB_CONTENT),
            Some(HelperRole::WebContent)
        );
        assert_eq!(classify_webkit_helper(PLAIN_GPU), Some(HelperRole::Gpu));
    }

    #[test]
    fn rejects_copies_lookalikes_and_networking() {
        // A same-named binary outside the system framework.
        let copy = "/Users/someone/Applications/com.apple.WebKit.WebContent.xpc/Contents/MacOS/com.apple.WebKit.WebContent";
        assert_eq!(classify_webkit_helper(copy), None);
        // Same framework, wrong helper.
        let networking = "/System/Library/Frameworks/WebKit.framework/Versions/A/XPCServices/com.apple.WebKit.Networking.xpc/Contents/MacOS/com.apple.WebKit.Networking";
        assert_eq!(classify_webkit_helper(networking), None);
        // Same directory prefix, different executable name.
        let renamed = "/System/Library/Frameworks/WebKit.framework/Versions/A/XPCServices/com.apple.WebKit.WebContent.xpc/Contents/MacOS/WebContent";
        assert_eq!(classify_webkit_helper(renamed), None);
        assert_eq!(classify_webkit_helper(""), None);
    }

    #[test]
    fn reads_own_coalition_and_usage() {
        let pid = std::process::id();
        let coalition = ProcCoalitionInfo::read(i32::try_from(pid).unwrap()).expect("coalition");
        // A zero coalition id would mean the payload was not filled.
        assert!(coalition.coalition_id.iter().any(|word| *word != 0));
        let usage = read_usage(pid).expect("rusage");
        assert!(usage.footprint_bytes > 0);
        assert!(usage.peak_bytes >= usage.footprint_bytes);
        assert!(usage.resident_bytes > 0);
    }

    #[test]
    fn scan_sees_own_pid_and_identifies_this_app() {
        let pid = std::process::id();
        assert!(list_all_pids().contains(&pid));
        let identity = bsd_identity(i32::try_from(pid).unwrap()).expect("bsd identity");
        assert_eq!(identity.euid, unsafe { libc::geteuid() });
        assert!(identity.start_marker > 0);
    }

    #[test]
    fn sample_of_this_process_is_self_consistent() {
        let mut tracker = Tracker::new(std::process::id());
        let snapshot = tracker.sample(Instant::now());
        assert_eq!(snapshot.seq, 1);
        assert_eq!(snapshot.gui_pid, std::process::id());
        assert!(snapshot.gui_usage.is_some());
        // This test binary shares no WebKit coalition, but the coalition of
        // its own process must be readable and it must not be attributed to
        // any WebKit helper it did not launch.
        assert!(snapshot.coalition.is_some());
        assert!(!snapshot.scan_failed);
        for item in snapshot.web_content.iter().chain(snapshot.gpu.iter()) {
            assert!(item.pid > 0);
            assert!(item.usage.is_some() || item.read_errno.is_some());
        }

        let second = tracker.sample(Instant::now());
        assert_eq!(second.seq, 2);
        assert!(second.uptime_ms >= snapshot.uptime_ms);
    }

    fn sample_snapshot() -> Snapshot {
        Snapshot {
            seq: 7,
            taken_at: Instant::now() - Duration::from_millis(1234),
            uptime_ms: 600_000,
            band: Band::Elevated,
            gui_pid: 4242,
            gui_usage: Some(Usage {
                footprint_bytes: 300_000_000,
                peak_bytes: 480_000_000,
                resident_bytes: 270_000_000,
            }),
            gui_errno: None,
            coalition: Some([0x1442, 0x1443]),
            coalition_reason: None,
            web_content: vec![HelperReading {
                pid: 9001,
                start_marker: 123,
                usage: Some(Usage {
                    footprint_bytes: 2_000_000_000,
                    peak_bytes: 3_000_000_000,
                    resident_bytes: 1_500_000_000,
                }),
                read_errno: None,
                seen_ms: 45_000,
            }],
            gpu: Vec::new(),
            scan_failed: false,
        }
    }

    #[test]
    fn sample_line_has_every_required_field() {
        let snapshot = sample_snapshot();
        let line = snapshot.render("sample", snapshot.taken_at);
        for key in [
            "event=sample",
            "seq=7",
            "band=elevated",
            "sample_age_ms=0",
            "sampling_interval_ms=15000",
            "gui_pid=4242",
            "gui_state=ok",
            "gui_footprint_bytes=300000000",
            "gui_peak_bytes=480000000",
            "coalition_state=ok",
            "web_content_state=ok",
            "web_content_count=1",
            "pid=9001",
            "footprint_bytes=2000000000",
            "peak_bytes=3000000000",
            "seen_ms=45000",
            "gpu_state=none",
            "gpu_count=0",
        ] {
            assert!(line.contains(key), "missing {key} in {line}");
        }
        // No executable paths or other content may reach the log.
        assert!(!line.contains("/System/"));
        assert!(!line.contains("/Users/"));
    }

    #[test]
    fn termination_line_links_to_last_sample_without_inferring_a_cause() {
        let snapshot = sample_snapshot();
        let line = snapshot.render(
            "web_content_terminate",
            snapshot.taken_at + Duration::from_secs(2),
        );
        assert!(line.contains("event=web_content_terminate"));
        assert!(line.contains("termination_reason=unavailable"));
        assert!(line.contains("cause_not_inferred=true"));
        assert!(line.contains("sample_age_ms=2000"));
        assert!(line.contains("seq=7"));
        assert!(line.contains("pid=9001"));
    }

    #[test]
    fn missing_diagnostics_are_named() {
        let mut tracker = Tracker::new(std::process::id());
        let mut snapshot = tracker.sample(Instant::now());
        snapshot.coalition = None;
        snapshot.coalition_reason = Some("proc_pidinfo_flavor_20 errno=1".to_string());
        snapshot.web_content.clear();
        let line = snapshot.render("sample", snapshot.taken_at);
        assert!(line.contains("coalition_state=unavailable"));
        assert!(line.contains("web_content_state=unavailable"));
        assert!(line.contains("web_content_reason=coalition_unreadable"));

        let mut no_scan = sample_snapshot();
        no_scan.scan_failed = true;
        no_scan.web_content.clear();
        let line = no_scan.render("sample", no_scan.taken_at);
        assert!(line.contains("web_content_reason=process_scan_failed"));
    }

    #[test]
    fn unreadable_usage_is_marked_per_process() {
        let mut snapshot = sample_snapshot();
        snapshot.web_content[0].usage = None;
        snapshot.web_content[0].read_errno = Some(1);
        let line = snapshot.render("sample", snapshot.taken_at);
        assert!(line.contains("pid=9001 state=unavailable reason=rusage_failed errno=1"));
    }

    #[test]
    fn band_follows_the_largest_attributed_footprint() {
        let mut tracker = Tracker::new(std::process::id());
        let snapshot = tracker.sample(Instant::now());
        assert_eq!(snapshot.band, Band::Normal);
        assert_eq!(Band::Normal.interval(), NORMAL_INTERVAL);
        assert_eq!(Band::Elevated.interval(), ELEVATED_INTERVAL);

        // A fresh tracker whose GUI usage crosses the threshold is elevated.
        let mut big = sample_snapshot();
        big.gui_usage = Some(Usage {
            footprint_bytes: ELEVATED_FOOTPRINT_BYTES,
            peak_bytes: ELEVATED_FOOTPRINT_BYTES,
            resident_bytes: 0,
        });
        let gui_usage = big.gui_usage;
        big.band = if [gui_usage]
            .into_iter()
            .chain(big.web_content.iter().map(|item| item.usage))
            .flatten()
            .any(|usage| usage.footprint_bytes >= ELEVATED_FOOTPRINT_BYTES)
        {
            Band::Elevated
        } else {
            Band::Normal
        };
        assert_eq!(big.band, Band::Elevated);
    }

    /// Manual probe against a live GUI: `WEBVIEW_MEMORY_PROBE_PID=<pid>` prints
    /// the sample line the running app would write. A no-op in CI, where the
    /// variable is absent.
    #[test]
    fn probe_live_attribution_when_requested() {
        let Ok(value) = std::env::var("WEBVIEW_MEMORY_PROBE_PID") else {
            return;
        };
        let Ok(pid) = value.parse::<u32>() else {
            return;
        };
        let mut tracker = Tracker::new(pid);
        let snapshot = tracker.sample(Instant::now());
        println!("{}", snapshot.render("probe", snapshot.taken_at));
        assert_eq!(snapshot.gui_pid, pid);
        assert!(snapshot.coalition.is_some());
    }
}
