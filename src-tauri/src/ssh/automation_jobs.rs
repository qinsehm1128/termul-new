//! Bounded, caller-owned automation jobs. Workers use the automation backend, never
//! user PTYs. Optional private snapshots retain only bounded results, never execution inputs.
use super::automation::{
    profile_fingerprint, AutomationBackend, AutomationError, AutomationResult, CallContext,
    Credentials, ExecRequest, ExecResult, ExecutionProgress,
};
use super::profile_manager::SSHProfile;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    fs::File,
    io::{Read, Write},
    panic::{catch_unwind, AssertUnwindSafe},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        Arc, Mutex, MutexGuard, Weak,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const MAX_JOBS: usize = 128;
const MAX_OUTPUT_BYTES: usize = 1024 * 1024;
const MAX_TOTAL_OUTPUT: usize = 16 * MAX_OUTPUT_BYTES;
const MAX_STORE_BYTES: usize = MAX_TOTAL_OUTPUT * 6 + 1024 * 1024;
const RETENTION: Duration = Duration::from_secs(15 * 60);
const GRANT_CHECK_INTERVAL: Duration = Duration::from_millis(250);
// Shared across managers, including workers whose manager has been dropped.
static JOB_SLOTS: AtomicUsize = AtomicUsize::new(0);

struct Slot;
impl Slot {
    fn acquire() -> AutomationResult<Self> {
        let mut current = JOB_SLOTS.load(Ordering::Acquire);
        loop {
            if current >= MAX_JOBS {
                return Err(AutomationError::new(
                    "job_limit",
                    "SSH job capacity has been reached",
                ));
            }
            match JOB_SLOTS.compare_exchange_weak(
                current,
                current + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Ok(Self),
                Err(observed) => current = observed,
            }
        }
    }
}
impl Drop for Slot {
    fn drop(&mut self) {
        JOB_SLOTS.fetch_sub(1, Ordering::AcqRel);
    }
}

struct Job {
    id: String,
    profile_id: String,
    profile_fingerprint: String,
    caller_id: String,
    // Recovered jobs never retain a GrantProvider or execution context.
    context: Option<CallContext>,
    quota: usize,
    cancel: Arc<AtomicBool>,
    data: Mutex<JobData>,
    _slot: Slot,
}
struct JobData {
    state: &'static str,
    output: String,
    stdout_bytes: usize,
    exit_code: Option<i32>,
    duration_ms: Option<u64>,
    truncated: bool,
    remote_termination_confirmed: bool,
    error_code: Option<&'static str>,
    finished_at: Option<Instant>,
    timestamp_ms: u64,
    streamed: bool,
    store_error: bool,
}
impl Job {
    fn request_cancel(&self) {
        let mut data = lock(&self.data);
        if data.finished_at.is_none() {
            self.cancel.store(true, Ordering::Release);
            data.state = "cancel_requested";
        }
    }

    fn finish(&self, result: AutomationResult<ExecResult>, quota: usize) {
        let mut data = lock(&self.data);
        match result {
            Ok(result) => {
                data.state = if result.cancel_confirmed {
                    "cancelled"
                } else if result.timed_out {
                    "timed_out"
                } else if result.exit_code == Some(0) {
                    "succeeded"
                } else {
                    "failed"
                };
                // Streaming callbacks already appended the authoritative arrival-order log.
                // Never append the backend's final aggregate again (or reorder stderr).
                if !data.streamed {
                    let stdout_end = floor_boundary(&result.stdout, result.stdout.len().min(quota));
                    data.output.push_str(&result.stdout[..stdout_end]);
                    data.stdout_bytes = stdout_end;
                    let remaining = quota - data.output.len();
                    let stderr_end =
                        floor_boundary(&result.stderr, result.stderr.len().min(remaining));
                    data.output.push_str(&result.stderr[..stderr_end]);
                    data.truncated = result.truncated
                        || stdout_end < result.stdout.len()
                        || stderr_end < result.stderr.len();
                } else {
                    data.truncated |= result.truncated;
                }
                data.exit_code = result.exit_code;
                data.duration_ms = Some(result.duration_ms);
                data.remote_termination_confirmed = result.remote_termination_confirmed;
            }
            Err(ref error) if error.code == "command_denied" || error.code == "POLICY_INVALID" => {
                data.state = "failed";
                data.error_code = Some(if error.code == "command_denied" {
                    "command_denied"
                } else {
                    "POLICY_INVALID"
                });
            }
            Err(_) => {
                // Backend messages may contain commands, paths or secrets. Never publish them.
                data.state = "failed";
                data.error_code = Some("ssh_error");
            }
        }
        data.finished_at = Some(Instant::now());
        data.timestamp_ms = now_ms();
    }

    fn value(&self, offset: usize, max_bytes: usize) -> AutomationResult<Value> {
        let data = lock(&self.data);
        if offset > data.output.len() || !data.output.is_char_boundary(offset) {
            return Err(AutomationError::invalid(
                "offset must be an output UTF-8 byte boundary",
            ));
        }
        let end = floor_boundary(
            &data.output,
            offset.saturating_add(max_bytes).min(data.output.len()),
        );
        if max_bytes > 0 && offset < data.output.len() && end == offset {
            return Err(AutomationError::invalid(
                "maxBytes is too small for the next UTF-8 character",
            ));
        }
        Ok(json!({
            "jobId": self.id, "profileId": self.profile_id, "state": data.state,
            "finished": data.finished_at.is_some(),
            "cancelRequested": self.cancel.load(Ordering::Acquire),
            "cancelConfirmed": data.state == "cancelled",
            "remoteTerminationConfirmed": data.remote_termination_confirmed,
            "exitCode": data.exit_code, "durationMs": data.duration_ms,
            "errorCode": data.error_code,
            "storeError": if data.store_error { Some("job_store_unavailable") } else { None },
            // A single bounded byte log, in callback arrival order. stdoutBytes is
            // a COUNT, not a contiguous stream boundary when progress is emitted.
            "output": &data.output[offset..end], "offset": offset, "nextOffset": end,
            "totalBytes": data.output.len(), "stdoutBytes": data.stdout_bytes,
            "truncated": data.truncated,
        }))
    }
}

/// At most 128 retained jobs process-wide; finished jobs expire after 15 minutes
/// or are evicted oldest-first under capacity pressure. Active jobs are never evicted.
/// A host must keep one manager for the lifetime of its automation service.
pub struct JobManager {
    jobs: Arc<Mutex<HashMap<String, Arc<Job>>>>,
    monitor_started: Mutex<bool>,
    store: Option<Arc<PrivateStore>>,
    progress_revision: AtomicU64,
    flushed_revision: AtomicU64,
}
impl Default for JobManager {
    fn default() -> Self {
        Self::new()
    }
}
impl JobManager {
    pub fn new() -> Self {
        Self {
            jobs: Arc::new(Mutex::new(HashMap::new())),
            monitor_started: Mutex::new(false),
            store: None,
            progress_revision: AtomicU64::new(0),
            flushed_revision: AtomicU64::new(0),
        }
    }

    /// Enable private atomic persistence. The immediate parent must already exist
    /// and be owned by this user with no group/world access. No real paths are
    /// selected implicitly: callers (and tests) explicitly inject the store path.
    pub fn with_store(path: PathBuf) -> AutomationResult<Self> {
        let store = PrivateStore::open(path)?;
        let records = store.load()?;
        let mut manager = Self::new();
        for record in records {
            let job = record.recover()?;
            lock(&manager.jobs).insert(job.id.clone(), Arc::new(job));
        }
        manager.store = Some(Arc::new(store));
        // Validate writeability and durably mark prior active work interrupted.
        manager.persist(true)?;
        Ok(manager)
    }

    fn persist(&self, force: bool) -> AutomationResult<()> {
        let Some(store) = &self.store else {
            return Ok(());
        };
        let revision = self.progress_revision.load(Ordering::Acquire);
        let result = store.snapshot(&self.jobs, force);
        if matches!(result, Ok(false)) {
            return Ok(());
        }
        for job in lock(&self.jobs).values() {
            lock(&job.data).store_error = result.is_err();
        }
        if result.is_ok() {
            self.flushed_revision.fetch_max(revision, Ordering::AcqRel);
        }
        result.map(|_| ())
    }

    /// Returns immediately; a dedicated blocking worker owns credentials until execute ends.
    /// Backend implementations must enforce the supplied timeout and output quota.
    pub fn start(
        self: &Arc<Self>,
        context: CallContext,
        profile: SSHProfile,
        credentials: Credentials,
        backend: Arc<dyn AutomationBackend>,
        mut request: ExecRequest,
    ) -> AutomationResult<Value> {
        context.authorize_command(&profile, "job_start", &request.command)?;
        self.authorize(&context, &profile.id, "job_start")?;
        let fingerprint = profile_fingerprint(&profile);
        let grant = context.authorize(&profile.id, "job_start")?;
        if grant.profile_fingerprint.as_deref() != Some(fingerprint.as_str()) {
            return Err(AutomationError::new(
                "profile_identity_changed",
                "SSH profile identity needs host authorization",
            ));
        }
        if request.max_bytes == 0 || request.max_bytes > MAX_OUTPUT_BYTES || request.timeout_ms == 0
        {
            return Err(AutomationError::invalid(
                "jobs require timeoutMs > 0 and maxBytes in 1..=1048576",
            ));
        }
        self.start_monitor()?;
        let quota = request.max_bytes;
        let id = uuid::Uuid::new_v4().to_string();
        // Neither a caller-provided token nor a caller-shared cancel flag is trusted.
        request.job_token = Some(id.clone());
        request.cancel = Arc::new(AtomicBool::new(false));
        let job = {
            let mut jobs = lock(&self.jobs);
            cleanup(&mut jobs);
            // Evict retained results only as necessary, never active workers.
            while jobs.values().map(|job| job.quota).sum::<usize>() + quota > MAX_TOTAL_OUTPUT {
                let oldest = jobs
                    .iter()
                    .filter_map(|(id, job)| lock(&job.data).finished_at.map(|at| (id.clone(), at)))
                    .min_by_key(|(_, at)| *at)
                    .map(|(id, _)| id);
                if let Some(id) = oldest {
                    jobs.remove(&id);
                } else {
                    return Err(AutomationError::new(
                        "job_limit",
                        "SSH job output capacity has been reached",
                    ));
                }
            }
            let slot = loop {
                match Slot::acquire() {
                    Ok(slot) => break slot,
                    Err(error) => {
                        let oldest = jobs
                            .iter()
                            .filter_map(|(id, job)| {
                                lock(&job.data).finished_at.map(|at| (id.clone(), at))
                            })
                            .min_by_key(|(_, at)| *at)
                            .map(|(id, _)| id);
                        match oldest {
                            Some(id) => {
                                jobs.remove(&id);
                            }
                            None => return Err(error),
                        }
                    }
                }
            };
            let job = Arc::new(Job {
                id: id.clone(),
                profile_id: profile.id.clone(),
                profile_fingerprint: fingerprint,
                caller_id: context.caller_id.clone(),
                context: Some(context),
                quota,
                cancel: request.cancel.clone(),
                _slot: slot,
                data: Mutex::new(JobData {
                    state: "running",
                    output: String::new(),
                    stdout_bytes: 0,
                    exit_code: None,
                    duration_ms: None,
                    truncated: false,
                    remote_termination_confirmed: false,
                    error_code: None,
                    finished_at: None,
                    timestamp_ms: now_ms(),
                    streamed: false,
                    store_error: false,
                }),
            });
            jobs.insert(id.clone(), job.clone());
            job
        };
        // Persist before any remote side effect; a failed start must not claim durability.
        if let Err(error) = self.persist(true) {
            lock(&self.jobs).remove(&id);
            return Err(error);
        }
        let worker_job = job.clone();
        let worker_store = self.store.clone();
        let worker_jobs = self.jobs.clone();
        let manager = Arc::downgrade(self);
        let progress: Arc<dyn ExecutionProgress> = Arc::new(JobProgress {
            job: Arc::downgrade(&job),
            manager: manager.clone(),
            quota,
        });
        let spawned = thread::Builder::new()
            .name("ssh-automation-job".into())
            .spawn(move || {
                let result = catch_unwind(AssertUnwindSafe(|| {
                    // Recheck immediately before any remote side effect.
                    let grant = worker_job
                        .context
                        .as_ref()
                        .ok_or_else(AutomationError::backend)?
                        .authorize_command(&profile, "job_start", &request.command)?;
                    if grant.profile_fingerprint.as_deref()
                        != Some(worker_job.profile_fingerprint.as_str())
                        || worker_job.cancel.load(Ordering::Acquire)
                    {
                        return Err(AutomationError::denied());
                    }
                    backend
                        .execute_with_progress(&profile, &credentials, request, progress)
                        .map_err(|_| AutomationError::backend())
                }))
                .unwrap_or_else(|_| Err(AutomationError::backend()));
                worker_job.finish(result, quota);
                if let Some(store) = worker_store {
                    // Always run final persistence, even after manager Drop.
                    // Errors are sanitized and observable without leaking paths.
                    let result = store.snapshot(&worker_jobs, true);
                    lock(&worker_job.data).store_error = result.is_err();
                }
            });
        if spawned.is_err() {
            lock(&self.jobs).remove(&id);
            self.persist(true)?;
            return Err(AutomationError::new(
                "job_unavailable",
                "SSH job worker could not start",
            ));
        }
        job.value(0, 0)
    }

    pub fn status(
        &self,
        context: &CallContext,
        profile_id: &str,
        args: &Value,
    ) -> AutomationResult<Value> {
        self.authorize(context, profile_id, "job_status")?;
        check_profile_arg(args, profile_id)?;
        let job_id = job_id_arg(args)?;
        let offset = number_arg(args, "offset", 0, 0, MAX_OUTPUT_BYTES)?;
        let max_bytes = number_arg(args, "maxBytes", 65536, 1, MAX_OUTPUT_BYTES)?;
        self.status_at(context, profile_id, job_id, offset, max_bytes)
    }

    pub fn list(
        &self,
        context: &CallContext,
        profile_id: &str,
        args: &Value,
    ) -> AutomationResult<Value> {
        self.authorize(context, profile_id, "job_list")?;
        check_profile_arg(args, profile_id)?;
        let offset = number_arg(args, "offset", 0, 0, MAX_JOBS)?;
        let limit = number_arg(args, "limit", 50, 1, MAX_JOBS)?;
        let state = match args.get("state") {
            None => None,
            Some(value) => {
                let state = value
                    .as_str()
                    .ok_or_else(|| AutomationError::invalid("state must be a string"))?;
                if ![
                    "running",
                    "succeeded",
                    "failed",
                    "timed_out",
                    "cancel_requested",
                    "cancelled",
                    "interrupted",
                ]
                .contains(&state)
                {
                    return Err(AutomationError::invalid("unknown job state"));
                }
                Some(state)
            }
        };
        let all = self.list_all(context, profile_id)?;
        let selected: Vec<_> = all["jobs"]
            .as_array()
            .expect("list_all returns jobs array")
            .iter()
            .filter(|job| state.is_none_or(|state| job["state"].as_str() == Some(state)))
            .collect();
        let total = selected.len();
        let jobs: Vec<_> = selected.into_iter().skip(offset).take(limit).collect();
        let next_offset = offset.saturating_add(jobs.len()).min(total);
        Ok(
            json!({ "profileId": profile_id, "jobs": jobs, "offset": offset,
            "nextOffset": next_offset, "total": total, "hasMore": next_offset < total }),
        )
    }

    pub fn cancel(
        &self,
        context: &CallContext,
        profile_id: &str,
        args: &Value,
    ) -> AutomationResult<Value> {
        self.authorize(context, profile_id, "job_cancel")?;
        check_profile_arg(args, profile_id)?;
        self.cancel_id(context, profile_id, job_id_arg(args)?)
    }

    fn status_at(
        &self,
        context: &CallContext,
        profile_id: &str,
        job_id: &str,
        offset: usize,
        max_bytes: usize,
    ) -> AutomationResult<Value> {
        self.authorize(context, profile_id, "job_status")?;
        if max_bytes == 0 || max_bytes > MAX_OUTPUT_BYTES {
            return Err(AutomationError::invalid("maxBytes must be in 1..=1048576"));
        }
        self.owned(context, profile_id, job_id)?
            .value(offset, max_bytes)
    }

    fn list_all(&self, context: &CallContext, profile_id: &str) -> AutomationResult<Value> {
        self.authorize(context, profile_id, "job_list")?;
        let mut jobs = lock(&self.jobs);
        cleanup(&mut jobs);
        let mut owned: Vec<_> = jobs
            .values()
            .filter(|job| job.caller_id == context.caller_id && job.profile_id == profile_id)
            .collect();
        owned.sort_by(|a, b| a.id.cmp(&b.id));
        let values: AutomationResult<Vec<_>> =
            owned.into_iter().map(|job| job.value(0, 0)).collect();
        Ok(json!({ "profileId": profile_id, "jobs": values? }))
    }

    fn cancel_id(
        &self,
        context: &CallContext,
        profile_id: &str,
        job_id: &str,
    ) -> AutomationResult<Value> {
        self.authorize(context, profile_id, "job_cancel")?;
        let job = self.owned(context, profile_id, job_id)?;
        if lock(&job.data).state == "interrupted" {
            return Err(AutomationError::new(
                "job_termination_unknown",
                "Recovered SSH work cannot be cancelled or its remote termination confirmed",
            ));
        }
        job.request_cancel();
        self.persist(true)?;
        job.value(0, 0)
    }

    fn owned(
        &self,
        context: &CallContext,
        profile_id: &str,
        job_id: &str,
    ) -> AutomationResult<Arc<Job>> {
        let mut jobs = lock(&self.jobs);
        cleanup(&mut jobs);
        jobs.get(job_id)
            .filter(|job| job.caller_id == context.caller_id && job.profile_id == profile_id)
            .cloned()
            .ok_or_else(|| AutomationError::new("job_not_found", "SSH job was not found"))
    }

    fn authorize(
        &self,
        context: &CallContext,
        profile_id: &str,
        operation: &str,
    ) -> AutomationResult<()> {
        let authorization = context.authorize(profile_id, operation).map(|_| ());
        if authorization.is_err() {
            // Revoked callers cannot access results, but their existing work is signalled.
            for job in lock(&self.jobs)
                .values()
                .filter(|job| job.caller_id == context.caller_id && job.profile_id == profile_id)
            {
                job.request_cancel();
            }
        }
        authorization
    }

    fn start_monitor(self: &Arc<Self>) -> AutomationResult<()> {
        let mut started = lock(&self.monitor_started);
        if *started {
            return Ok(());
        }
        let weak = Arc::downgrade(self);
        if thread::Builder::new()
            .name("ssh-job-grants".into())
            .spawn(move || loop {
                thread::sleep(GRANT_CHECK_INTERVAL);
                let Some(manager) = weak.upgrade() else { break };
                // Flush a last/single progress chunk even if no later callback
                // arrives. Revisions avoid losing concurrent dirty notifications.
                if manager.progress_revision.load(Ordering::Acquire)
                    != manager.flushed_revision.load(Ordering::Acquire)
                {
                    let _ = manager.persist(false);
                }
                let jobs: Vec<_> = {
                    let mut entries = lock(&manager.jobs);
                    cleanup(&mut entries);
                    entries.values().map(Arc::downgrade).collect()
                };
                // Do not hold the manager or registry lock while calling host policy code.
                let weak_manager = Arc::downgrade(&manager);
                drop(manager);
                for weak_job in jobs {
                    let Some(job) = weak_job.upgrade() else {
                        continue;
                    };
                    if lock(&job.data).finished_at.is_some() {
                        continue;
                    }
                    let Some(context) = job.context.clone() else {
                        continue;
                    };
                    let profile_id = job.profile_id.clone();
                    let fingerprint = job.profile_fingerprint.clone();
                    drop(job);
                    let granted = catch_unwind(AssertUnwindSafe(|| {
                        context.authorize(&profile_id, "job_start").map(|grant| {
                            grant.profile_fingerprint.as_deref() == Some(fingerprint.as_str())
                        })
                    }))
                    .ok()
                    .and_then(Result::ok)
                    .unwrap_or(false);
                    if !granted {
                        if let Some(job) = weak_job.upgrade() {
                            job.request_cancel();
                            if let Some(manager) = weak_manager.upgrade() {
                                let _ = manager.persist(false);
                            }
                        }
                    }
                }
            })
            .is_err()
        {
            return Err(AutomationError::new(
                "job_unavailable",
                "SSH grant monitor could not start",
            ));
        }
        *started = true;
        Ok(())
    }
}
impl Drop for JobManager {
    fn drop(&mut self) {
        for job in lock(&self.jobs).values() {
            job.request_cancel();
        }
        let _ = self.persist(true);
    }
}

struct JobProgress {
    job: Weak<Job>,
    manager: Weak<JobManager>,
    quota: usize,
}
impl ExecutionProgress for JobProgress {
    fn output(&self, stdout: bool, chunk: &[u8]) {
        let Some(job) = self.job.upgrade() else {
            return;
        };
        {
            let mut data = lock(&job.data);
            if data.finished_at.is_some() || chunk.is_empty() {
                return;
            }
            data.streamed = true;
            let remaining = self.quota.saturating_sub(data.output.len());
            // Limit the input slice BEFORE lossy conversion: malformed bytes can
            // expand threefold. The stored UTF-8 log never exceeds its quota.
            let input = &chunk[..chunk.len().min(remaining)];
            let text = String::from_utf8_lossy(input);
            let end = floor_boundary(&text, text.len().min(remaining));
            data.output.push_str(&text[..end]);
            if stdout {
                data.stdout_bytes += end;
            }
            data.truncated |= input.len() < chunk.len() || end < text.len();
        }
        if let Some(manager) = self.manager.upgrade() {
            manager.progress_revision.fetch_add(1, Ordering::AcqRel);
            let _ = manager.persist(false);
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}
fn store_error() -> AutomationError {
    // Never return raw filesystem errors (which can contain private paths).
    AutomationError::new(
        "job_store_unavailable",
        "Private SSH job store is unavailable or unsafe",
    )
}

// Explicit allowlist: no command, credentials, GrantProvider, raw error, PID or
// reconnect material can enter this representation. Refuse unknown fields on load.
#[derive(Clone, Copy, Serialize, Deserialize)]
enum StoredJobError {
    #[serde(rename = "command_denied")]
    CommandDenied,
    #[serde(rename = "POLICY_INVALID")]
    PolicyInvalid,
    #[serde(rename = "ssh_error")]
    SshError,
}
impl StoredJobError {
    fn as_str(self) -> &'static str {
        match self {
            Self::CommandDenied => "command_denied",
            Self::PolicyInvalid => "POLICY_INVALID",
            Self::SshError => "ssh_error",
        }
    }
    fn from_code(code: &str) -> Option<Self> {
        match code {
            "command_denied" => Some(Self::CommandDenied),
            "POLICY_INVALID" => Some(Self::PolicyInvalid),
            "ssh_error" => Some(Self::SshError),
            _ => None,
        }
    }
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredJob {
    id: String,
    caller_id: String,
    profile_id: String,
    profile_fingerprint: String,
    state: String,
    output: String,
    stdout_bytes: usize,
    exit_code: Option<i32>,
    duration_ms: Option<u64>,
    truncated: bool,
    cancel_requested: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error_code: Option<StoredJobError>,
    remote_termination_confirmed: bool,
    timestamp_ms: u64,
}
impl StoredJob {
    fn from_job(job: &Job) -> Self {
        let data = lock(&job.data);
        Self {
            id: job.id.clone(),
            caller_id: job.caller_id.clone(),
            profile_id: job.profile_id.clone(),
            profile_fingerprint: job.profile_fingerprint.clone(),
            state: data.state.into(),
            output: data.output.clone(),
            stdout_bytes: data.stdout_bytes,
            exit_code: data.exit_code,
            duration_ms: data.duration_ms,
            truncated: data.truncated,
            cancel_requested: job.cancel.load(Ordering::Acquire),
            error_code: data.error_code.and_then(StoredJobError::from_code),
            remote_termination_confirmed: data.remote_termination_confirmed,
            timestamp_ms: data.timestamp_ms,
        }
    }
    fn valid(&self) -> bool {
        [
            &self.id,
            &self.caller_id,
            &self.profile_id,
            &self.profile_fingerprint,
        ]
        .iter()
        .all(|text| !text.is_empty() && text.len() <= 256)
            && self.output.len() <= MAX_OUTPUT_BYTES
            && self.stdout_bytes <= self.output.len()
            && [
                "active",
                "running",
                "cancel_requested",
                "interrupted",
                "succeeded",
                "failed",
                "timed_out",
                "cancelled",
            ]
            .contains(&self.state.as_str())
            && self.timestamp_ms <= now_ms()
    }
    fn recover(self) -> AutomationResult<Job> {
        let state = match self.state.as_str() {
            "succeeded" => "succeeded",
            "failed" => "failed",
            "timed_out" => "timed_out",
            "cancelled" => "cancelled",
            _ => "interrupted",
        };
        let age = Duration::from_millis(now_ms().saturating_sub(self.timestamp_ms));
        let interrupted = state == "interrupted";
        Ok(Job {
            id: self.id,
            caller_id: self.caller_id,
            profile_id: self.profile_id,
            profile_fingerprint: self.profile_fingerprint,
            context: None,
            quota: self.output.len().max(1),
            cancel: Arc::new(AtomicBool::new(self.cancel_requested)),
            _slot: Slot::acquire()?,
            data: Mutex::new(JobData {
                state,
                output: self.output,
                stdout_bytes: self.stdout_bytes,
                exit_code: if interrupted { None } else { self.exit_code },
                duration_ms: self.duration_ms,
                truncated: self.truncated,
                remote_termination_confirmed: !interrupted && self.remote_termination_confirmed,
                error_code: if interrupted {
                    None
                } else {
                    self.error_code.map(StoredJobError::as_str)
                },
                finished_at: Some(Instant::now() - age.min(RETENTION)),
                timestamp_ms: self.timestamp_ms,
                streamed: false,
                store_error: false,
            }),
        })
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoreDocument {
    version: u8,
    jobs: Vec<StoredJob>,
}

#[cfg(unix)]
struct PrivateStore {
    // An anchored directory descriptor prevents ancestor substitution after open.
    directory: File,
    name: std::ffi::CString,
    writer: Mutex<Option<Instant>>,
    // Never unlink the sidecar: replacing its inode would split the OS lock.
    // Workers retain this lease through their final snapshot after manager Drop.
    _lease: File,
}
#[cfg(not(unix))]
struct PrivateStore;

#[cfg(unix)]
impl PrivateStore {
    fn open(path: PathBuf) -> AutomationResult<Self> {
        use std::{
            ffi::CString,
            os::unix::{
                ffi::OsStrExt,
                io::{AsRawFd, FromRawFd},
            },
            path::Component,
        };
        let path = if path.is_absolute() {
            path
        } else {
            std::env::current_dir()
                .map_err(|_| store_error())?
                .join(path)
        };
        let parent = path.parent().ok_or_else(store_error)?;
        let name = CString::new(path.file_name().ok_or_else(store_error)?.as_bytes())
            .map_err(|_| store_error())?;
        let mut directory = File::open("/").map_err(|_| store_error())?;
        let components: Vec<_> = parent.components().collect();
        for (index, component) in components.iter().enumerate() {
            let Component::Normal(part) = component else {
                if matches!(component, Component::RootDir | Component::CurDir) {
                    continue;
                }
                return Err(store_error());
            };
            let part = CString::new(part.as_bytes()).map_err(|_| store_error())?;
            let fd = unsafe {
                libc::openat(
                    directory.as_raw_fd(),
                    part.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                )
            };
            if fd < 0 {
                return Err(store_error());
            }
            directory = unsafe { File::from_raw_fd(fd) };
            Self::check_directory(&directory, index + 1 == components.len())?;
        }
        // Also enforce this for paths directly under root.
        Self::check_directory(&directory, true)?;
        let lease = Self::acquire_lease(&directory, &name)?;
        let store = Self {
            directory,
            name,
            writer: Mutex::new(None),
            _lease: lease,
        };
        store.read_file()?; // Reject unsafe existing destination before use.
        Ok(store)
    }
    fn acquire_lease(directory: &File, name: &std::ffi::CStr) -> AutomationResult<File> {
        use std::{
            ffi::CString,
            os::unix::{
                fs::{MetadataExt, PermissionsExt},
                io::{AsRawFd, FromRawFd},
            },
        };
        let mut bytes = name.to_bytes().to_vec();
        bytes.extend_from_slice(b".lock");
        let name = CString::new(bytes).map_err(|_| store_error())?;
        let flags = libc::O_RDWR | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC;
        let mut fd = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                name.as_ptr(),
                flags | libc::O_CREAT | libc::O_EXCL,
                0o600,
            )
        };
        let created = fd >= 0;
        if fd < 0 {
            if std::io::Error::last_os_error().kind() != std::io::ErrorKind::AlreadyExists {
                return Err(store_error());
            }
            fd = unsafe { libc::openat(directory.as_raw_fd(), name.as_ptr(), flags) };
        }
        if fd < 0 {
            return Err(store_error());
        }
        let lease = unsafe { File::from_raw_fd(fd) };
        if created {
            lease
                .set_permissions(std::fs::Permissions::from_mode(0o600))
                .map_err(|_| store_error())?;
        }
        let meta = lease.metadata().map_err(|_| store_error())?;
        if !meta.is_file()
            || meta.uid() != unsafe { libc::geteuid() }
            || meta.mode() & 0o077 != 0
            || meta.nlink() != 1
            || meta.len() != 0
        {
            return Err(store_error());
        }
        // flock locks independent file descriptions, including another manager
        // in this same process as well as desktop/standalone writer processes.
        if unsafe { libc::flock(lease.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return if std::io::Error::last_os_error().kind() == std::io::ErrorKind::WouldBlock {
                Err(AutomationError::new(
                    "store_busy",
                    "Private SSH job store already has an active writer",
                ))
            } else {
                Err(store_error())
            };
        }
        Ok(lease)
    }
    fn check_directory(file: &File, immediate: bool) -> AutomationResult<()> {
        use std::os::unix::fs::MetadataExt;
        let meta = file.metadata().map_err(|_| store_error())?;
        let uid = unsafe { libc::geteuid() };
        // Root-owned sticky system temp directories may be traversed, but must
        // never be the immediate store parent. A private fixture lives below one.
        let sticky_root = meta.uid() == 0 && meta.mode() & 0o1000 != 0;
        if !meta.is_dir()
            || (meta.uid() != uid && meta.uid() != 0)
            || (meta.mode() & 0o022 != 0 && !sticky_root)
            || (immediate && (meta.uid() != uid || meta.mode() & 0o077 != 0))
        {
            return Err(store_error());
        }
        Ok(())
    }
    fn read_file(&self) -> AutomationResult<Option<File>> {
        use std::os::unix::{
            fs::MetadataExt,
            io::{AsRawFd, FromRawFd},
        };
        let fd = unsafe {
            libc::openat(
                self.directory.as_raw_fd(),
                self.name.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return if std::io::Error::last_os_error().kind() == std::io::ErrorKind::NotFound {
                Ok(None)
            } else {
                Err(store_error())
            };
        }
        let file = unsafe { File::from_raw_fd(fd) };
        let meta = file.metadata().map_err(|_| store_error())?;
        if !meta.is_file()
            || meta.uid() != unsafe { libc::geteuid() }
            || meta.mode() & 0o077 != 0
            || meta.nlink() != 1
            || meta.len() > MAX_STORE_BYTES as u64
        {
            return Err(store_error());
        }
        Ok(Some(file))
    }
    fn load(&self) -> AutomationResult<Vec<StoredJob>> {
        let Some(file) = self.read_file()? else {
            return Ok(Vec::new());
        };
        let mut bytes = Vec::new();
        file.take(MAX_STORE_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| store_error())?;
        if bytes.len() > MAX_STORE_BYTES {
            return Err(store_error());
        }
        let document: StoreDocument = serde_json::from_slice(&bytes).map_err(|_| store_error())?;
        let mut ids = std::collections::HashSet::new();
        let mut total = 0usize;
        if document.version != 1 || document.jobs.len() > MAX_JOBS {
            return Err(store_error());
        }
        for job in &document.jobs {
            total = total.saturating_add(job.output.len());
            if !job.valid() || !ids.insert(&job.id) || total > MAX_TOTAL_OUTPUT {
                return Err(store_error());
            }
        }
        Ok(document
            .jobs
            .into_iter()
            .filter(|job| now_ms().saturating_sub(job.timestamp_ms) < RETENTION.as_millis() as u64)
            .collect())
    }
    fn snapshot(
        &self,
        jobs: &Mutex<HashMap<String, Arc<Job>>>,
        force: bool,
    ) -> AutomationResult<bool> {
        use std::{
            ffi::CString,
            os::unix::io::{AsRawFd, FromRawFd},
        };
        let mut writer = lock(&self.writer);
        if !force && writer.is_some_and(|at| at.elapsed() < GRANT_CHECK_INTERVAL) {
            return Ok(false);
        }
        // Throttle attempts as well as successes to bound I/O when storage fails.
        *writer = Some(Instant::now());
        Self::check_directory(&self.directory, true)?;
        self.read_file()?;
        let records = {
            let mut entries = lock(jobs);
            cleanup(&mut entries);
            entries
                .values()
                .map(|job| StoredJob::from_job(job))
                .collect::<Vec<_>>()
        };
        if records.len() > MAX_JOBS
            || records.iter().any(|job| !job.valid())
            || records.iter().map(|job| job.output.len()).sum::<usize>() > MAX_TOTAL_OUTPUT
        {
            return Err(store_error());
        }
        let bytes = serde_json::to_vec(&StoreDocument {
            version: 1,
            jobs: records,
        })
        .map_err(|_| store_error())?;
        if bytes.len() > MAX_STORE_BYTES {
            return Err(store_error());
        }
        let temporary = CString::new(format!(".ssh-jobs-{}.tmp", uuid::Uuid::new_v4()))
            .map_err(|_| store_error())?;
        let fd = unsafe {
            libc::openat(
                self.directory.as_raw_fd(),
                temporary.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                0o600,
            )
        };
        if fd < 0 {
            return Err(store_error());
        }
        let mut file = unsafe { File::from_raw_fd(fd) };
        let result = (|| {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))
                .map_err(|_| store_error())?;
            file.write_all(&bytes).map_err(|_| store_error())?;
            file.sync_all().map_err(|_| store_error())?;
            self.read_file()?;
            if unsafe {
                libc::renameat(
                    self.directory.as_raw_fd(),
                    temporary.as_ptr(),
                    self.directory.as_raw_fd(),
                    self.name.as_ptr(),
                )
            } != 0
            {
                return Err(store_error());
            }
            self.directory.sync_all().map_err(|_| store_error())
        })();
        if result.is_err() {
            unsafe {
                libc::unlinkat(self.directory.as_raw_fd(), temporary.as_ptr(), 0);
            }
        }
        result.map(|_| true)
    }
}
#[cfg(not(unix))]
impl PrivateStore {
    fn open(_: PathBuf) -> AutomationResult<Self> {
        Err(store_error())
    }
    fn load(&self) -> AutomationResult<Vec<StoredJob>> {
        Err(store_error())
    }
    fn snapshot(&self, _: &Mutex<HashMap<String, Arc<Job>>>, _: bool) -> AutomationResult<bool> {
        Err(store_error())
    }
}

fn check_profile_arg(args: &Value, profile_id: &str) -> AutomationResult<()> {
    if args.as_object().is_none()
        || args.get("profileId").and_then(Value::as_str) != Some(profile_id)
    {
        return Err(AutomationError::invalid(
            "profileId must match the requested profile",
        ));
    }
    Ok(())
}
fn job_id_arg(args: &Value) -> AutomationResult<&str> {
    args.get("jobId")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty() && id.len() <= 256)
        .ok_or_else(|| {
            AutomationError::invalid("jobId must be a nonempty string of at most 256 bytes")
        })
}
fn number_arg(
    args: &Value,
    name: &str,
    default: usize,
    min: usize,
    max: usize,
) -> AutomationResult<usize> {
    let Some(value) = args.get(name) else {
        return Ok(default);
    };
    value
        .as_u64()
        .and_then(|value| usize::try_from(value).ok())
        .filter(|value| *value >= min && *value <= max)
        .ok_or_else(|| AutomationError::invalid("job pagination argument is out of range"))
}

fn cleanup(jobs: &mut HashMap<String, Arc<Job>>) {
    jobs.retain(|_, job| {
        lock(&job.data)
            .finished_at
            .is_none_or(|at| at.elapsed() < RETENTION)
    });
}
fn floor_boundary(value: &str, mut end: usize) -> usize {
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    end
}
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // Worker panics must not poison ownership checks or prevent cancellation.
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::super::automation::{GrantProvider, ProfileGrant, SshPolicy};
    use super::*;
    static SERIAL: Mutex<()> = Mutex::new(());

    struct Policy(Mutex<SshPolicy>);
    impl GrantProvider for Policy {
        fn policy(&self, _: &str) -> AutomationResult<SshPolicy> {
            Ok(lock(&self.0).clone())
        }
    }
    struct FakeBackend {
        result: ExecResult,
        wait_for_cancel: bool,
        entered: AtomicBool,
        fail: bool,
        stream: bool,
    }
    impl AutomationBackend for FakeBackend {
        fn execute(
            &self,
            _: &SSHProfile,
            credentials: &Credentials,
            request: ExecRequest,
        ) -> AutomationResult<ExecResult> {
            assert_eq!(credentials.secret.as_deref(), Some("private-credential"));
            assert!(uuid::Uuid::parse_str(request.job_token.as_deref().unwrap()).is_ok());
            assert!(request.max_bytes <= MAX_OUTPUT_BYTES);
            self.entered.store(true, Ordering::Release);
            if self.wait_for_cancel {
                let deadline = Instant::now() + Duration::from_secs(3);
                while !request.cancel.load(Ordering::Acquire) {
                    assert!(Instant::now() < deadline, "cancellation was not signalled");
                    thread::sleep(Duration::from_millis(5));
                }
            }
            if self.fail {
                Err(AutomationError::new(
                    "sensitive",
                    "private-credential command-secret",
                ))
            } else {
                Ok(self.result.clone())
            }
        }
        fn execute_with_progress(
            &self,
            profile: &SSHProfile,
            credentials: &Credentials,
            request: ExecRequest,
            progress: Arc<dyn ExecutionProgress>,
        ) -> AutomationResult<ExecResult> {
            if self.stream {
                progress.output(false, b"err-");
                progress.output(true, b"out");
            }
            self.execute(profile, credentials, request)
        }
        fn files(
            &self,
            _: &SSHProfile,
            _: &Credentials,
            _: &str,
            _: &Value,
            _: &ProfileGrant,
        ) -> AutomationResult<Value> {
            unreachable!()
        }
        fn test_connection(&self, _: &SSHProfile, _: &Credentials) -> AutomationResult<Value> {
            unreachable!()
        }
    }
    fn result() -> ExecResult {
        ExecResult {
            stdout: "hello".into(),
            stderr: "err".into(),
            exit_code: Some(0),
            duration_ms: 1,
            truncated: false,
            timed_out: false,
            cancel_confirmed: false,
            remote_termination_confirmed: true,
        }
    }
    fn fake(result: ExecResult, wait: bool) -> Arc<FakeBackend> {
        Arc::new(FakeBackend {
            result,
            wait_for_cancel: wait,
            entered: AtomicBool::new(false),
            fail: false,
            stream: false,
        })
    }
    fn fixture() -> (Arc<JobManager>, CallContext, Arc<Policy>) {
        let mut policy = SshPolicy::default();
        let grant = ProfileGrant {
            operations: ["job_start", "job_status", "job_list", "job_cancel"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            posix_jobs: true,
            profile_fingerprint: Some(profile_fingerprint(&profile())),
            ..ProfileGrant::default()
        };
        policy.profiles.insert("p".into(), grant.clone());
        policy.profiles.insert("other".into(), grant);
        let policy = Arc::new(Policy(Mutex::new(policy)));
        (
            Arc::new(JobManager::new()),
            CallContext {
                caller_id: "alice".into(),
                grants: policy.clone(),
            },
            policy,
        )
    }
    fn profile() -> SSHProfile {
        serde_json::from_value(json!({
            "id":"p", "name":"test", "host":"invalid.example", "port":22,
            "username":"test", "authMethod":"agent", "portForwards":[],
        }))
        .unwrap()
    }
    fn request(max_bytes: usize) -> ExecRequest {
        ExecRequest {
            command: "command-secret".into(),
            timeout_ms: 500,
            max_bytes,
            job_token: Some("untrusted-token".into()),
            cancel: Arc::new(AtomicBool::new(true)),
        }
    }
    fn start(
        manager: &Arc<JobManager>,
        context: &CallContext,
        backend: Arc<FakeBackend>,
        quota: usize,
    ) -> String {
        manager
            .start(
                context.clone(),
                profile(),
                Credentials {
                    secret: Some("private-credential".into()),
                },
                backend,
                request(quota),
            )
            .unwrap()["jobId"]
            .as_str()
            .unwrap()
            .to_owned()
    }
    fn until(mut predicate: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(4);
        while !predicate() {
            assert!(Instant::now() < deadline, "test timed out");
            thread::sleep(Duration::from_millis(5));
        }
    }
    fn finished(manager: &JobManager, context: &CallContext, id: &str) -> Value {
        until(|| {
            manager
                .status_at(context, "p", id, 0, MAX_OUTPUT_BYTES)
                .unwrap()["finished"]
                == true
        });
        manager
            .status_at(context, "p", id, 0, MAX_OUTPUT_BYTES)
            .unwrap()
    }
    // Let the monitor release any snapshot and dropped workers release their global slot.
    fn drained() {
        until(|| JOB_SLOTS.load(Ordering::Acquire) == 0);
    }

    #[cfg(unix)]
    struct StoreFixture(PathBuf);
    #[cfg(unix)]
    impl StoreFixture {
        fn new() -> Self {
            use std::os::unix::fs::DirBuilderExt;
            let path = std::env::temp_dir()
                .canonicalize()
                .unwrap()
                .join(format!("termul-ssh-jobs-test-{}", uuid::Uuid::new_v4()));
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(&path)
                .unwrap();
            Self(path)
        }
        fn path(&self) -> PathBuf {
            self.0.join("jobs.json")
        }
        fn write(&self, path: &std::path::Path, bytes: &[u8]) {
            use std::os::unix::fs::OpenOptionsExt;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(path)
                .unwrap();
            file.write_all(bytes).unwrap();
        }
    }
    #[cfg(unix)]
    impl Drop for StoreFixture {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[cfg(unix)]
    fn stored_record(id: usize, state: &str, output: String) -> StoredJob {
        StoredJob {
            id: format!("fixture-job-{id}"),
            caller_id: "alice".into(),
            profile_id: "p".into(),
            profile_fingerprint: "fingerprint-before-edit".into(),
            state: state.into(),
            stdout_bytes: output.len(),
            output,
            exit_code: None,
            duration_ms: None,
            truncated: false,
            cancel_requested: state == "cancel_requested",
            error_code: None,
            remote_termination_confirmed: false,
            timestamp_ms: now_ms(),
        }
    }

    #[cfg(unix)]
    #[test]
    fn trusted_command_denial_survives_storage_roundtrip() {
        let mut record = stored_record(1, "failed", String::new());
        record.error_code = Some(StoredJobError::CommandDenied);
        let encoded = serde_json::to_vec(&record).unwrap();
        let recovered: StoredJob = serde_json::from_slice(&encoded).unwrap();
        let job = recovered.recover().unwrap();
        assert_eq!(job.value(0, 0).unwrap()["errorCode"], "command_denied");
        let mut malformed = serde_json::to_value(&record).unwrap();
        malformed["errorCode"] = json!("untrusted arbitrary backend text");
        assert!(serde_json::from_value::<StoredJob>(malformed).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn store_lease_child_probe() {
        let Some(path) = std::env::var_os("TERMUL_SSH_JOB_STORE_LEASE_PROBE") else {
            return;
        };
        assert_eq!(
            JobManager::with_store(PathBuf::from(path))
                .err()
                .unwrap()
                .code,
            "store_busy"
        );
    }

    #[cfg(unix)]
    #[test]
    fn store_lease_excludes_other_managers_and_processes_and_releases_on_drop() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let _serial = lock(&SERIAL);
        drained();
        let fixture = StoreFixture::new();
        let path = fixture.path();
        let manager = JobManager::with_store(path.clone()).unwrap();
        assert_eq!(
            JobManager::with_store(path.clone()).err().unwrap().code,
            "store_busy"
        );
        let sidecar = fixture.0.join("jobs.json.lock");
        assert_eq!(
            std::fs::metadata(&sidecar).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "ssh::automation_jobs::tests::store_lease_child_probe",
                "--nocapture",
            ])
            .env("TERMUL_SSH_JOB_STORE_LEASE_PROBE", &path)
            .status()
            .unwrap();
        assert!(child.success());
        drop(manager);
        assert!(sidecar.exists()); // Persistent inode; never unlink to unlock.
        drop(JobManager::with_store(path.clone()).unwrap());
        std::fs::set_permissions(&sidecar, std::fs::Permissions::from_mode(0o640)).unwrap();
        assert_eq!(
            JobManager::with_store(path.clone()).err().unwrap().code,
            "job_store_unavailable"
        );
        std::fs::set_permissions(&sidecar, std::fs::Permissions::from_mode(0o600)).unwrap();
        let unsafe_path = fixture.0.join("symlink-store.json");
        symlink(&sidecar, fixture.0.join("symlink-store.json.lock")).unwrap();
        assert!(JobManager::with_store(unsafe_path).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn store_rejects_unsafe_files_and_parents_without_following_symlinks() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let _serial = lock(&SERIAL);
        drained();
        let fixture = StoreFixture::new();
        let empty = br#"{"version":1,"jobs":[]}"#;
        fixture.write(&fixture.path(), empty);
        std::fs::set_permissions(fixture.path(), std::fs::Permissions::from_mode(0o640)).unwrap();
        assert!(JobManager::with_store(fixture.path()).is_err());
        std::fs::set_permissions(fixture.path(), std::fs::Permissions::from_mode(0o600)).unwrap();
        let link = fixture.0.join("linked.json");
        symlink(fixture.path(), &link).unwrap();
        assert!(JobManager::with_store(link).is_err());
        let directory = fixture.0.join("not-regular.json");
        std::fs::create_dir(&directory).unwrap();
        assert!(JobManager::with_store(directory).is_err());
        let alias = fixture.0.join("parent-link");
        symlink(&fixture.0, &alias).unwrap();
        assert!(JobManager::with_store(alias.join("jobs.json")).is_err());
        let unsafe_parent = fixture.0.join("unsafe-parent");
        std::fs::create_dir(&unsafe_parent).unwrap();
        std::fs::set_permissions(&unsafe_parent, std::fs::Permissions::from_mode(0o777)).unwrap();
        let private_child = unsafe_parent.join("child");
        std::fs::create_dir(&private_child).unwrap();
        std::fs::set_permissions(&private_child, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(JobManager::with_store(private_child.join("jobs.json")).is_err());
        std::fs::set_permissions(&fixture.0, std::fs::Permissions::from_mode(0o750)).unwrap();
        assert!(JobManager::with_store(fixture.path()).is_err());
        std::fs::set_permissions(&fixture.0, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(std::fs::read(fixture.path()).unwrap(), empty);
    }

    #[cfg(unix)]
    #[test]
    fn store_load_bounds_ttl_and_all_prior_active_states_are_enforced() {
        let _serial = lock(&SERIAL);
        drained();
        let fixture = StoreFixture::new();
        fixture.write(&fixture.path(), br#"{"version":1,"jobs":[]}"#);
        let replace = |records: Vec<StoredJob>| {
            std::fs::write(
                fixture.path(),
                serde_json::to_vec(&StoreDocument {
                    version: 1,
                    jobs: records,
                })
                .unwrap(),
            )
            .unwrap();
        };
        replace(
            (0..MAX_JOBS + 1)
                .map(|id| stored_record(id, "running", String::new()))
                .collect(),
        );
        assert!(PrivateStore::open(fixture.path()).unwrap().load().is_err());
        replace(vec![stored_record(
            0,
            "succeeded",
            "x".repeat(MAX_OUTPUT_BYTES + 1),
        )]);
        assert!(PrivateStore::open(fixture.path()).unwrap().load().is_err());
        replace(
            (0..17)
                .map(|id| stored_record(id, "succeeded", "x".repeat(MAX_OUTPUT_BYTES)))
                .collect(),
        );
        assert!(PrivateStore::open(fixture.path()).unwrap().load().is_err());
        let mut expired = stored_record(0, "succeeded", "expired".into());
        expired.timestamp_ms -= RETENTION.as_millis() as u64 + 1;
        replace(vec![expired]);
        assert!(JobManager::with_store(fixture.path())
            .unwrap()
            .jobs
            .lock()
            .unwrap()
            .is_empty());
        for state in ["active", "running", "cancel_requested"] {
            replace(vec![stored_record(0, state, "retained".into())]);
            let manager = JobManager::with_store(fixture.path()).unwrap();
            let (_, context, _) = self::fixture();
            let status = manager
                .status_at(&context, "p", "fixture-job-0", 0, 100)
                .unwrap();
            assert_eq!(status["state"], "interrupted");
            assert_eq!(status["output"], "retained");
            assert_eq!(status["cancelConfirmed"], false);
            assert_eq!(status["remoteTerminationConfirmed"], false);
        }
        replace(Vec::new());
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(fixture.path())
            .unwrap();
        file.set_len(MAX_STORE_BYTES as u64 + 1).unwrap();
        assert!(PrivateStore::open(fixture.path()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn persistence_failures_are_explicit_and_worker_status_is_sanitized() {
        use std::os::unix::fs::PermissionsExt;
        let _serial = lock(&SERIAL);
        drained();
        let fixture = StoreFixture::new();
        let (_, context, _) = self::fixture();
        let manager = Arc::new(JobManager::with_store(fixture.path()).unwrap());
        std::fs::set_permissions(fixture.path(), std::fs::Permissions::from_mode(0o644)).unwrap();
        let backend = fake(result(), true);
        let error = manager
            .start(
                context.clone(),
                profile(),
                Credentials { secret: None },
                backend.clone(),
                request(128),
            )
            .unwrap_err();
        assert_eq!(error.code, "job_store_unavailable");
        assert!(!backend.entered.load(Ordering::Acquire));
        assert!(lock(&manager.jobs).is_empty());
        std::fs::set_permissions(fixture.path(), std::fs::Permissions::from_mode(0o600)).unwrap();
        let id = start(&manager, &context, backend.clone(), 128);
        until(|| backend.entered.load(Ordering::Acquire));
        std::fs::set_permissions(fixture.path(), std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(
            manager.cancel_id(&context, "p", &id).unwrap_err().code,
            "job_store_unavailable"
        );
        finished(&manager, &context, &id);
        until(|| {
            manager.status_at(&context, "p", &id, 0, 128).unwrap()["storeError"]
                == "job_store_unavailable"
        });
        let status = manager
            .status_at(&context, "p", &id, 0, 128)
            .unwrap()
            .to_string();
        assert!(!status.contains(fixture.0.to_str().unwrap()));
        std::fs::set_permissions(fixture.path(), std::fs::Permissions::from_mode(0o600)).unwrap();
        manager.persist(true).unwrap();
        assert!(manager.status_at(&context, "p", &id, 0, 128).unwrap()["storeError"].is_null());
    }

    #[cfg(unix)]
    #[test]
    fn running_progress_is_ordered_and_final_is_not_duplicated() {
        let _serial = lock(&SERIAL);
        drained();
        let fixture = StoreFixture::new();
        let (_, context, _) = self::fixture();
        let backend = Arc::new(FakeBackend {
            result: result(),
            wait_for_cancel: true,
            entered: AtomicBool::new(false),
            fail: false,
            stream: true,
        });
        let manager = Arc::new(JobManager::with_store(fixture.path()).unwrap());
        let id = start(&manager, &context, backend.clone(), 64);
        until(|| backend.entered.load(Ordering::Acquire));
        let running = manager.status_at(&context, "p", &id, 0, 64).unwrap();
        assert_eq!(running["state"], "running");
        assert_eq!(running["totalBytes"], 7);
        assert_eq!(running["stdoutBytes"], 3); // Count, not a boundary.
        assert_eq!(running["output"], "err-out");
        assert_eq!(
            manager.status_at(&context, "p", &id, 4, 3).unwrap()["output"],
            "out"
        );
        // A burst is throttled; the next interval publishes bounded output.
        let first_snapshot = *lock(&manager.store.as_ref().unwrap().writer);
        let job = manager.owned(&context, "p", &id).unwrap();
        let sink = JobProgress {
            job: Arc::downgrade(&job),
            manager: Arc::downgrade(&manager),
            quota: 64,
        };
        sink.output(true, b"!");
        assert_eq!(
            *lock(&manager.store.as_ref().unwrap().writer),
            first_snapshot
        );
        until(|| {
            let saved: Value =
                serde_json::from_slice(&std::fs::read(fixture.path()).unwrap()).unwrap();
            saved["jobs"][0]["output"] == "err-out!"
        });
        sink.output(false, b"?");
        until(|| {
            let saved: Value =
                serde_json::from_slice(&std::fs::read(fixture.path()).unwrap()).unwrap();
            saved["jobs"][0]["output"] == "err-out!?"
        });
        manager.cancel_id(&context, "p", &id).unwrap();
        let finished = finished(&manager, &context, &id);
        assert_eq!(finished["totalBytes"], 9);
        assert_eq!(finished["output"], "err-out!?");
        manager.persist(true).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn worker_final_snapshot_survives_manager_drop() {
        let _serial = lock(&SERIAL);
        drained();
        let fixture = StoreFixture::new();
        let (_, context, _) = self::fixture();
        let manager = Arc::new(JobManager::with_store(fixture.path()).unwrap());
        let mut result = result();
        result.cancel_confirmed = true;
        result.remote_termination_confirmed = true;
        let backend = fake(result, true);
        let id = start(&manager, &context, backend.clone(), 128);
        until(|| backend.entered.load(Ordering::Acquire));
        drop(manager);
        drained(); // Worker released its slot only after final snapshot finished.
        let recovered = JobManager::with_store(fixture.path()).unwrap();
        let status = recovered.status_at(&context, "p", &id, 0, 128).unwrap();
        assert_eq!(status["state"], "cancelled");
        assert_eq!(status["cancelConfirmed"], true);
        assert_eq!(status["remoteTerminationConfirmed"], true);
    }

    #[cfg(unix)]
    #[test]
    fn restart_retains_jobs_but_never_reconnects_or_confirms_interrupted_cancel() {
        use std::os::unix::fs::PermissionsExt;
        let _serial = lock(&SERIAL);
        drained();
        let fixture = StoreFixture::new();
        let (_, context, policy) = self::fixture();
        let manager = Arc::new(JobManager::with_store(fixture.path()).unwrap());
        let backend = fake(result(), true);
        let id = start(&manager, &context, backend.clone(), 128);
        until(|| backend.entered.load(Ordering::Acquire));
        manager.persist(true).unwrap();
        let bytes = std::fs::read(fixture.path()).unwrap();
        let saved: Value = serde_json::from_slice(&bytes).unwrap();
        for field in [
            "command",
            "context",
            "credentials",
            "grants",
            "error",
            "pid",
        ] {
            assert!(saved["jobs"][0].get(field).is_none());
        }
        assert!(!String::from_utf8_lossy(&bytes).contains("command-secret"));
        assert!(!String::from_utf8_lossy(&bytes).contains("private-credential"));
        assert_eq!(
            std::fs::metadata(fixture.path())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        let recovered_path = fixture.0.join("recovered.json");
        fixture.write(&recovered_path, &bytes);
        manager.cancel_id(&context, "p", &id).unwrap();
        finished(&manager, &context, &id);
        manager.persist(true).unwrap();
        drop(manager);

        let recovered = JobManager::with_store(recovered_path).unwrap();
        let status = recovered.status_at(&context, "p", &id, 0, 128).unwrap();
        assert_eq!(status["state"], "interrupted");
        assert_eq!(status["remoteTerminationConfirmed"], false);
        assert_eq!(status["cancelConfirmed"], false);
        assert!(lock(&recovered.jobs).get(&id).unwrap().context.is_none());
        assert_eq!(
            recovered.list_all(&context, "p").unwrap()["jobs"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            recovered.cancel_id(&context, "p", &id).unwrap_err().code,
            "job_termination_unknown"
        );
        let mut other = context.clone();
        other.caller_id = "other".into();
        assert_eq!(
            recovered
                .status_at(&other, "p", &id, 0, 128)
                .unwrap_err()
                .code,
            "job_not_found"
        );
        assert!(recovered.status_at(&context, "other", &id, 0, 128).is_err());
        let granted = lock(&policy.0).profiles["p"].operations.clone();
        lock(&policy.0)
            .profiles
            .get_mut("p")
            .unwrap()
            .operations
            .clear();
        assert!(recovered.status_at(&context, "p", &id, 0, 128).is_err());
        lock(&policy.0).profiles.get_mut("p").unwrap().operations = granted;
        let terminal = JobManager::with_store(fixture.path()).unwrap();
        assert_eq!(
            terminal.status_at(&context, "p", &id, 0, 128).unwrap()["state"],
            "succeeded"
        );
    }

    #[test]
    fn progress_lossy_expansion_never_exceeds_quota() {
        let _serial = lock(&SERIAL);
        drained();
        let (manager, context, _) = fixture();
        let backend = fake(result(), true);
        let id = start(&manager, &context, backend.clone(), 5);
        until(|| backend.entered.load(Ordering::Acquire));
        let job = manager.owned(&context, "p", &id).unwrap();
        let sink = JobProgress {
            job: Arc::downgrade(&job),
            manager: Arc::downgrade(&manager),
            quota: 5,
        };
        sink.output(true, &vec![0xff; MAX_OUTPUT_BYTES * 2]);
        sink.output(false, b"ab");
        assert_eq!(lock(&job.data).output, "\u{fffd}ab");
        assert_eq!(lock(&job.data).stdout_bytes, 3);
        assert!(lock(&job.data).truncated);
        manager.cancel_id(&context, "p", &id).unwrap();
        assert_eq!(finished(&manager, &context, &id)["totalBytes"], 5);
    }

    #[test]
    fn results_pagination_and_metadata_are_bounded_and_private() {
        let _serial = lock(&SERIAL);
        drained();
        let (manager, context, _) = fixture();
        let mut output = result();
        output.stdout = "aébc".into();
        output.stderr = "xyz".into();
        let id = start(&manager, &context, fake(output, false), 6);
        let value = finished(&manager, &context, &id);
        assert_eq!(value["state"], "succeeded");
        assert_eq!(value["output"], "aébcx");
        assert_eq!(value["totalBytes"], 6);
        assert_eq!(value["stdoutBytes"], 5);
        assert_eq!(value["truncated"], true);
        let page = manager.status_at(&context, "p", &id, 0, 2).unwrap();
        assert_eq!(page["output"], "a");
        assert_eq!(page["nextOffset"], 1);
        assert_eq!(
            manager.status_at(&context, "p", &id, 1, 2).unwrap()["output"],
            "é"
        );
        assert!(manager.status_at(&context, "p", &id, 2, 2).is_err());
        assert!(manager.status_at(&context, "p", &id, 1, 1).is_err());
        assert!(manager.status_at(&context, "p", &id, 7, 1).is_err());
        assert!(manager
            .status_at(&context, "p", &id, 0, MAX_OUTPUT_BYTES + 1)
            .is_err());
        let listing = manager.list_all(&context, "p").unwrap();
        assert_eq!(listing["jobs"][0]["output"], "");
        assert!(!listing.to_string().contains("command-secret"));
        assert!(!value.to_string().contains("private-credential"));
    }

    #[test]
    fn ownership_profile_scope_and_live_grants_are_enforced() {
        let _serial = lock(&SERIAL);
        drained();
        let (manager, context, policy) = fixture();
        let id = start(&manager, &context, fake(result(), false), 100);
        finished(&manager, &context, &id);
        let mut bob = context.clone();
        bob.caller_id = "bob".into();
        assert_eq!(
            manager.status_at(&bob, "p", &id, 0, 100).unwrap_err().code,
            "job_not_found"
        );
        assert!(manager.cancel_id(&bob, "p", &id).is_err());
        assert_eq!(
            manager.list_all(&bob, "p").unwrap()["jobs"]
                .as_array()
                .unwrap()
                .len(),
            0
        );
        assert!(manager.status_at(&context, "other", &id, 0, 100).is_err());
        assert!(manager.cancel_id(&context, "other", &id).is_err());
        lock(&policy.0).profiles.clear();
        assert_eq!(
            manager
                .status_at(&context, "p", &id, 0, 100)
                .unwrap_err()
                .code,
            "permission_denied"
        );
        assert!(manager.list_all(&context, "p").is_err());
        assert!(manager.cancel_id(&context, "p", &id).is_err());
    }

    #[test]
    fn cancellation_needs_backend_confirmation() {
        let _serial = lock(&SERIAL);
        drained();
        for confirmed in [false, true] {
            let (manager, context, _) = fixture();
            let mut output = result();
            output.exit_code = None;
            output.cancel_confirmed = confirmed;
            output.remote_termination_confirmed = confirmed;
            let backend = fake(output, true);
            let id = start(&manager, &context, backend.clone(), 100);
            until(|| backend.entered.load(Ordering::Acquire));
            let cancelled = manager.cancel_id(&context, "p", &id).unwrap();
            assert_eq!(cancelled["cancelRequested"], true);
            assert!(cancelled["state"] == "cancel_requested" || cancelled["finished"] == true);
            let value = finished(&manager, &context, &id);
            assert_eq!(
                value["state"],
                if confirmed { "cancelled" } else { "failed" }
            );
            assert_eq!(value["cancelConfirmed"], confirmed);
            assert_eq!(value["remoteTerminationConfirmed"], confirmed);
        }
    }

    #[test]
    fn monitor_cancels_revoked_work_without_a_followup_call() {
        let _serial = lock(&SERIAL);
        drained();
        let (manager, context, policy) = fixture();
        let backend = fake(result(), true);
        let id = start(&manager, &context, backend.clone(), 100);
        until(|| backend.entered.load(Ordering::Acquire));
        lock(&policy.0)
            .profiles
            .get_mut("p")
            .unwrap()
            .operations
            .clear();
        until(|| {
            lock(&manager.jobs)
                .get(&id)
                .unwrap()
                .cancel
                .load(Ordering::Acquire)
        });
        until(|| {
            lock(&lock(&manager.jobs).get(&id).unwrap().data)
                .finished_at
                .is_some()
        });
        assert!(manager.status_at(&context, "p", &id, 0, 100).is_err());
    }

    #[test]
    fn timeout_and_backend_error_do_not_claim_cancellation_or_leak_errors() {
        let _serial = lock(&SERIAL);
        drained();
        let (manager, context, _) = fixture();
        let mut output = result();
        output.timed_out = true;
        output.remote_termination_confirmed = false;
        let id = start(&manager, &context, fake(output, false), 100);
        assert_eq!(finished(&manager, &context, &id)["state"], "timed_out");
        let backend = Arc::new(FakeBackend {
            result: result(),
            wait_for_cancel: false,
            entered: AtomicBool::new(false),
            fail: true,
            stream: false,
        });
        let id = start(&manager, &context, backend, 100);
        let value = finished(&manager, &context, &id);
        assert_eq!(value["state"], "failed");
        assert_eq!(value["errorCode"], "ssh_error");
        assert!(!value.to_string().contains("private-credential"));
        assert!(!value.to_string().contains("command-secret"));
    }

    #[test]
    fn rejects_revoked_start_and_invalid_limits_before_launch() {
        let _serial = lock(&SERIAL);
        drained();
        let (manager, context, policy) = fixture();
        for quota in [0, MAX_OUTPUT_BYTES + 1] {
            assert!(manager
                .start(
                    context.clone(),
                    profile(),
                    Credentials::default(),
                    fake(result(), false),
                    request(quota)
                )
                .is_err());
        }
        lock(&policy.0)
            .profiles
            .get_mut("p")
            .unwrap()
            .operations
            .remove("job_start");
        assert_eq!(
            manager
                .start(
                    context,
                    profile(),
                    Credentials::default(),
                    fake(result(), false),
                    request(100)
                )
                .unwrap_err()
                .code,
            "permission_denied"
        );
        assert!(lock(&manager.jobs).is_empty());
    }

    #[test]
    fn global_capacity_retention_and_terminal_eviction_are_bounded() {
        let _serial = lock(&SERIAL);
        drained();
        let (manager, context, _) = fixture();
        // Model active entries without launching 128 threads or any network calls.
        for n in 0..MAX_JOBS {
            let id = n.to_string();
            lock(&manager.jobs).insert(
                id.clone(),
                Arc::new(Job {
                    id,
                    profile_id: "p".into(),
                    profile_fingerprint: profile_fingerprint(&profile()),
                    caller_id: context.caller_id.clone(),
                    context: Some(context.clone()),
                    quota: 100,
                    cancel: Arc::new(AtomicBool::new(false)),
                    _slot: Slot::acquire().unwrap(),
                    data: Mutex::new(JobData {
                        state: "running",
                        output: String::new(),
                        stdout_bytes: 0,
                        exit_code: None,
                        duration_ms: None,
                        truncated: false,
                        remote_termination_confirmed: false,
                        error_code: None,
                        finished_at: None,
                        timestamp_ms: now_ms(),
                        streamed: false,
                        store_error: false,
                    }),
                }),
            );
        }
        assert_eq!(Slot::acquire().err().unwrap().code, "job_limit");
        assert_eq!(
            manager
                .start(
                    context.clone(),
                    profile(),
                    Credentials::default(),
                    fake(result(), false),
                    request(100)
                )
                .unwrap_err()
                .code,
            "job_limit"
        );
        lock(&lock(&manager.jobs).get("0").unwrap().data).finished_at = Some(Instant::now());
        let id = start(&manager, &context, fake(result(), false), 100);
        finished(&manager, &context, &id);
        assert_eq!(lock(&manager.jobs).len(), MAX_JOBS);
        assert!(!lock(&manager.jobs).contains_key("0"));
        lock(&lock(&manager.jobs).get(&id).unwrap().data).finished_at =
            Some(Instant::now() - RETENTION);
        assert!(manager.status_at(&context, "p", &id, 0, 100).is_err());
        assert_eq!(lock(&manager.jobs).len(), MAX_JOBS - 1);
    }

    #[test]
    fn service_argument_api_and_filtered_list_pagination() {
        let _serial = lock(&SERIAL);
        drained();
        let (manager, context, _) = fixture();
        let id = start(&manager, &context, fake(result(), false), 100);
        finished(&manager, &context, &id);
        let mut failed = result();
        failed.exit_code = Some(1);
        let failed_id = start(&manager, &context, fake(failed, false), 100);
        finished(&manager, &context, &failed_id);
        let value = manager
            .status(
                &context,
                "p",
                &json!({"profileId":"p", "jobId":id, "offset":1, "maxBytes":2}),
            )
            .unwrap();
        assert_eq!(value["output"], "el");
        assert_eq!(
            manager
                .cancel(&context, "p", &json!({"profileId":"p", "jobId":id}))
                .unwrap()["state"],
            "succeeded"
        );
        let page = manager
            .list(&context, "p", &json!({"profileId":"p", "limit":1}))
            .unwrap();
        assert_eq!(page["jobs"].as_array().unwrap().len(), 1);
        assert_eq!(page["total"], 2);
        assert_eq!(page["nextOffset"], 1);
        assert_eq!(page["hasMore"], true);
        let second = manager
            .list(
                &context,
                "p",
                &json!({"profileId":"p", "offset":1, "limit":1}),
            )
            .unwrap();
        assert_ne!(page["jobs"][0]["jobId"], second["jobs"][0]["jobId"]);
        assert_eq!(second["hasMore"], false);
        let filtered = manager
            .list(&context, "p", &json!({"profileId":"p", "state":"failed"}))
            .unwrap();
        assert_eq!(filtered["total"], 1);
        assert_eq!(filtered["jobs"][0]["jobId"], failed_id);
        for args in [
            json!({}),
            json!({"profileId":"other"}),
            json!({"profileId":"p","offset":-1}),
            json!({"profileId":"p","limit":0}),
            json!({"profileId":"p","state":"bogus"}),
            json!({"profileId":"p","state":null}),
        ] {
            assert!(manager.list(&context, "p", &args).is_err());
        }
        assert!(manager
            .status(&context, "p", &json!({"profileId":"p"}))
            .is_err());
        assert!(manager
            .status(
                &context,
                "p",
                &json!({"profileId":"p","jobId":id,"maxBytes":0})
            )
            .is_err());
        assert!(manager
            .cancel(&context, "p", &json!({"profileId":"p","jobId":""}))
            .is_err());
    }

    #[test]
    fn fingerprint_change_cancels_work_but_keeps_old_job_management_access() {
        let _serial = lock(&SERIAL);
        drained();
        for changed in [None, Some("replacement-identity".to_owned())] {
            let (manager, context, policy) = fixture();
            let backend = fake(result(), true);
            let id = start(&manager, &context, backend.clone(), 100);
            until(|| backend.entered.load(Ordering::Acquire));
            lock(&policy.0)
                .profiles
                .get_mut("p")
                .unwrap()
                .profile_fingerprint = changed;
            until(|| {
                lock(&manager.jobs)
                    .get(&id)
                    .unwrap()
                    .cancel
                    .load(Ordering::Acquire)
            });
            assert!(manager
                .status(&context, "p", &json!({"profileId":"p","jobId":id}))
                .is_ok());
            assert!(manager
                .list(&context, "p", &json!({"profileId":"p"}))
                .is_ok());
            assert!(manager
                .cancel(&context, "p", &json!({"profileId":"p","jobId":id}))
                .is_ok());
            finished(&manager, &context, &id);
            assert_eq!(
                manager
                    .start(
                        context.clone(),
                        profile(),
                        Credentials::default(),
                        fake(result(), false),
                        request(100)
                    )
                    .unwrap_err()
                    .code,
                "profile_identity_changed"
            );
            // A profile edited independently of a still-authorized old grant also cannot start.
            lock(&policy.0)
                .profiles
                .get_mut("p")
                .unwrap()
                .profile_fingerprint = Some(profile_fingerprint(&profile()));
            let mut edited = profile();
            edited.host = "replacement.example".into();
            assert_eq!(
                manager
                    .start(
                        context,
                        edited,
                        Credentials::default(),
                        fake(result(), false),
                        request(100)
                    )
                    .unwrap_err()
                    .code,
                "profile_identity_changed"
            );
        }
    }

    #[test]
    fn dropping_manager_signals_workers() {
        let _serial = lock(&SERIAL);
        drained();
        let (manager, context, _) = fixture();
        let backend = fake(result(), true);
        start(&manager, &context, backend.clone(), 100);
        until(|| backend.entered.load(Ordering::Acquire));
        drop(manager);
        drained();
    }
}
