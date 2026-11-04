//! Host-owned, ticketed storage. Broker entry points never perform filesystem I/O.
//! Data is deliberately independent of the installation registry and survives uninstall.
use crate::registry::{finish_persist, owner_only_directory, read_optional, reject_non_regular};
use se_foundation::durable_fs::{
    CrashInjector, CrashPoint, DirectoryPermissions, DurableFileSystem,
};
use se_plugin_protocol::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};
use uuid::Uuid;

type Result<T> = std::result::Result<T, PluginError>;
const MAX_KEYS: usize = 256;
const MAX_PLUGIN_BYTES: usize = 1024 * 1024;
const MAX_GENERATION_PENDING: usize = 8;
const MAX_GLOBAL_PENDING: usize = 64;
static PENDING: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum StorageRequest {
    Get(StorageGetRequest),
    Set(StorageSetRequest),
    Delete(StorageDeleteRequest),
}
impl StorageRequest {
    fn validate(&self) -> Result<()> {
        match self {
            Self::Get(v) => validate_storage_get_request(v),
            Self::Set(v) => validate_storage_set_request(v),
            Self::Delete(v) => validate_storage_delete_request(v),
        }
    }
    fn identity(&self) -> (&str, &str, Option<&str>) {
        match self {
            Self::Get(v) => (&v.request_id, &v.key, v.project_id.as_deref()),
            Self::Set(v) => (&v.request_id, &v.key, v.project_id.as_deref()),
            Self::Delete(v) => (&v.request_id, &v.key, v.project_id.as_deref()),
        }
    }
}

#[derive(Debug)]
pub(crate) struct StorageCompletion {
    pub(crate) plugin_id: String,
    pub(crate) generation: String,
    pub(crate) input: StorageCompletionInput,
}

#[derive(Default)]
struct Lease {
    active: AtomicBool,
    withdrawn: AtomicBool,
}
struct Generation {
    id: String,
    lease: Arc<Lease>,
    pending: BTreeMap<String, Admission>,
    recent: VecDeque<Recent>,
    tokens: f64,
    refill: Instant,
    queue: VecDeque<Job>,
}
struct Admission {
    destination: Option<String>,
    request: StorageRequest,
    ticket: StorageTicket,
    deadline: Instant,
    timed_out: bool,
    worker_done: bool,
    delivery_finished: bool,
    abandoned: bool,
}
struct Recent {
    destination: Option<String>,
    request: StorageRequest,
    ticket: StorageTicket,
    input: StorageCompletionInput,
}
struct Permit;
impl Drop for Permit {
    fn drop(&mut self) {
        PENDING.fetch_sub(1, Ordering::AcqRel);
    }
}
struct Job {
    plugin: String,
    generation: String,
    lease: Arc<Lease>,
    request: StorageRequest,
    ticket: StorageTicket,
    deadline: Instant,
    _permit: Permit,
}
struct Ready {
    completion: StorageCompletion,
    lease: Arc<Lease>,
    _permit: Option<Permit>,
}
#[derive(Default)]
struct State {
    generations: BTreeMap<String, Generation>,
    busy: BTreeSet<String>,
    ready: VecDeque<Ready>,
    inflight: BTreeMap<String, Ready>,
}
struct Inner {
    root: PathBuf,
    state: Mutex<State>,
    #[cfg(test)]
    fault: Mutex<Option<CrashPoint>>,
    #[cfg(test)]
    withdraw_at: Mutex<Option<CrashPoint>>,
}

/// One profile's storage service. The host retains its profile lock for this lifetime.
/// `register` is valid during create; `activate` releases its queue after initialization.
/// The host must authorize `host.storage` and resolve project existence before `accept`.
/// Pump completions on the same serialized lifecycle lane as generation withdrawal.
pub(crate) struct Storage {
    inner: Arc<Inner>,
}
impl Storage {
    /// `profile_root` is the profile directory, NOT its `plugins` subdirectory.
    pub(crate) fn new(profile_root: PathBuf) -> Self {
        let _ = pool();
        Self {
            inner: Arc::new(Inner {
                root: profile_root.join("plugins").join("data"),
                state: Mutex::new(State::default()),
                #[cfg(test)]
                fault: Mutex::new(None),
                #[cfg(test)]
                withdraw_at: Mutex::new(None),
            }),
        }
    }
    pub(crate) fn register(&self, plugin: &str, generation: &str) -> Result<()> {
        if !is_plugin_id(plugin) || !is_uuid(generation) {
            return Err(error(
                PLUGIN_INVALID_REQUEST,
                "invalid storage generation identity",
            ));
        }
        let mut state = self.inner.state.lock().unwrap();
        if state.generations.contains_key(plugin) {
            return Err(error(PLUGIN_BUSY, "storage generation already registered"));
        }
        state.generations.insert(
            plugin.into(),
            Generation {
                id: generation.into(),
                lease: Arc::new(Lease::default()),
                pending: BTreeMap::new(),
                recent: VecDeque::new(),
                tokens: 20.0,
                refill: Instant::now(),
                queue: VecDeque::new(),
            },
        );
        Ok(())
    }
    pub(crate) fn activate(&self, plugin: &str, generation: &str) -> Result<()> {
        let mut state = self.inner.state.lock().unwrap();
        let live = state
            .generations
            .get(plugin)
            .filter(|v| v.id == generation)
            .ok_or_else(stale)?;
        live.lease.active.store(true, Ordering::Release);
        schedule(&self.inner, &mut state);
        Ok(())
    }
    /// Cancels queued/unpublished work and drops undelivered completions, without I/O.
    pub(crate) fn revoke(&self, plugin: &str, generation: &str) {
        let mut state = self.inner.state.lock().unwrap();
        if state
            .generations
            .get(plugin)
            .is_some_and(|v| v.id == generation)
        {
            let old = state.generations.remove(plugin).unwrap();
            old.lease.withdrawn.store(true, Ordering::Release);
            let mut pool = pool().state.lock().unwrap();
            let before = pool.queue.len();
            pool.queue
                .retain(|work| !Arc::ptr_eq(&work.job.lease, &old.lease));
            if pool.queue.len() != before {
                state.busy.remove(plugin);
            }
            state.ready.retain(|v| !Arc::ptr_eq(&v.lease, &old.lease));
            state
                .inflight
                .retain(|_, v| !Arc::ptr_eq(&v.lease, &old.lease));
        }
    }
    pub(crate) fn accept(
        &self,
        plugin: &str,
        generation: &str,
        request: StorageRequest,
    ) -> Result<StorageTicket> {
        self.accept_destination(plugin, generation, request, None)
    }
    pub(crate) fn accept_destination(
        &self,
        plugin: &str,
        generation: &str,
        request: StorageRequest,
        destination: Option<&str>,
    ) -> Result<StorageTicket> {
        request.validate()?;
        let mut state = self.inner.state.lock().unwrap();
        let live = state
            .generations
            .get_mut(plugin)
            .filter(|v| v.id == generation)
            .ok_or_else(stale)?;
        let request_id = request.identity().0.to_owned();
        if let Some(prior) = live.pending.get(&request_id) {
            return if prior.request == request && prior.destination.as_deref() == destination {
                if prior.abandoned {
                    Err(error(
                        PLUGIN_BUSY,
                        "abandoned storage worker is still running",
                    ))
                } else {
                    Ok(prior.ticket.clone())
                }
            } else {
                Err(error(
                    PLUGIN_INVALID_REQUEST,
                    "storage requestId payload changed",
                ))
            };
        }
        let previous = live
            .recent
            .iter()
            .find(|v| v.ticket.request_id == request_id);
        if previous.is_some_and(|v| v.request != request || v.destination.as_deref() != destination)
        {
            return Err(error(
                PLUGIN_INVALID_REQUEST,
                "storage requestId payload changed",
            ));
        }
        if live.pending.len() >= MAX_GENERATION_PENDING {
            return Err(error(PLUGIN_BUSY, "storage generation queue is full"));
        }
        let now = Instant::now();
        let tokens = (live.tokens + now.duration_since(live.refill).as_secs_f64() * 10.0).min(20.0);
        if tokens < 1.0 {
            return Err(error(PLUGIN_BUSY, "storage rate limit exceeded"));
        }
        PENDING
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n < MAX_GLOBAL_PENDING).then_some(n + 1)
            })
            .map_err(|_| error(PLUGIN_BUSY, "storage global queue is full"))?;
        live.tokens = tokens - 1.0;
        live.refill = now;
        let ticket = previous
            .map(|v| v.ticket.clone())
            .unwrap_or_else(|| StorageTicket {
                accepted: true,
                ticket_id: Uuid::new_v4().to_string(),
                request_id,
            });
        let cached = previous.map(|v| v.input.clone());
        let deadline = now + Duration::from_secs(30);
        live.pending.insert(
            ticket.request_id.clone(),
            Admission {
                destination: destination.map(str::to_owned),
                request: request.clone(),
                ticket: ticket.clone(),
                deadline,
                timed_out: false,
                worker_done: cached.is_some(),
                delivery_finished: false,
                abandoned: false,
            },
        );
        let lease = live.lease.clone();
        if let Some(input) = cached {
            state.ready.push_back(Ready {
                completion: StorageCompletion {
                    plugin_id: plugin.into(),
                    generation: generation.into(),
                    input,
                },
                lease,
                _permit: Some(Permit),
            });
        } else {
            live.queue.push_back(Job {
                plugin: plugin.into(),
                generation: generation.into(),
                lease: live.lease.clone(),
                request,
                ticket: ticket.clone(),
                deadline,
                _permit: Permit,
            });
        }
        schedule(&self.inner, &mut state);
        Ok(ticket)
    }
    pub(crate) fn pending_count(&self) -> usize {
        self.inner
            .state
            .lock()
            .unwrap()
            .generations
            .values()
            .map(|live| live.pending.len())
            .sum()
    }
    /// Discards a web wait without turning it into a native completion. Work that may
    /// still mutate the filesystem retains its admission and permit until `run` ends.
    pub(crate) fn abandon_destination(&self, plugin: &str, generation: &str, destination: &str) {
        let mut state = self.inner.state.lock().unwrap();
        let Some(live) = state
            .generations
            .get_mut(plugin)
            .filter(|v| v.id == generation)
        else {
            return;
        };
        let mut tickets = BTreeSet::new();
        for prior in live.pending.values_mut() {
            if prior.destination.as_deref() == Some(destination) {
                prior.abandoned = true;
                tickets.insert(prior.ticket.ticket_id.clone());
            }
        }
        live.pending
            .retain(|_, prior| !(prior.abandoned && prior.worker_done));
        state
            .ready
            .retain(|v| !tickets.contains(&v.completion.input.ticket_id));
        state.inflight.retain(|ticket, _| !tickets.contains(ticket));
    }

    /// Reserves a completion for this generation without releasing its admission slot.
    /// The host must acknowledge successful callback delivery or return it for retry.
    pub(crate) fn pop_completion_for(
        &self,
        plugin: &str,
        generation: &str,
    ) -> Option<StorageCompletion> {
        self.pop_completion_destination(plugin, generation, None)
    }
    pub(crate) fn pop_completion_destination(
        &self,
        plugin: &str,
        generation: &str,
        destination: Option<&str>,
    ) -> Option<StorageCompletion> {
        let mut state = self.inner.state.lock().unwrap();
        let live = state.generations.get(plugin).filter(|v| {
            v.id == generation
                && v.lease.active.load(Ordering::Acquire)
                && !v.lease.withdrawn.load(Ordering::Acquire)
        })?;
        let lease = live.lease.clone();
        let index = state.ready.iter().position(|v| {
            Arc::ptr_eq(&v.lease, &lease)
                && live
                    .pending
                    .get(&v.completion.input.request_id)
                    .is_some_and(|a| !a.abandoned && a.destination.as_deref() == destination)
        });
        let ready = if let Some(index) = index {
            let mut ready = state.ready.remove(index).unwrap();
            let live = state.generations.get_mut(plugin).unwrap();
            let prior = live.pending.get_mut(&ready.completion.input.request_id)?;
            if ready._permit.is_some() {
                prior.worker_done = true;
            }
            if Instant::now() >= prior.deadline {
                ready.completion.input.result = timeout_result();
            }
            ready
        } else {
            let live = state.generations.get_mut(plugin).unwrap();
            let prior = live.pending.values_mut().find(|v| {
                !v.abandoned
                    && v.destination.as_deref() == destination
                    && !v.timed_out
                    && !v.worker_done
                    && Instant::now() >= v.deadline
            })?;
            prior.timed_out = true;
            Ready {
                completion: StorageCompletion {
                    plugin_id: plugin.into(),
                    generation: generation.into(),
                    input: StorageCompletionInput {
                        request_id: prior.ticket.request_id.clone(),
                        ticket_id: prior.ticket.ticket_id.clone(),
                        result: timeout_result(),
                    },
                },
                lease,
                _permit: None,
            }
        };
        let completion = StorageCompletion {
            plugin_id: ready.completion.plugin_id.clone(),
            generation: ready.completion.generation.clone(),
            input: ready.completion.input.clone(),
        };
        state
            .inflight
            .insert(completion.input.ticket_id.clone(), ready);
        Some(completion)
    }
    /// Returns an unaccepted callback delivery while retaining its original quota permit.
    pub(crate) fn return_completion(&self, completion: StorageCompletion) {
        let mut state = self.inner.state.lock().unwrap();
        let valid = state
            .generations
            .get(&completion.plugin_id)
            .is_some_and(|v| {
                v.id == completion.generation && !v.lease.withdrawn.load(Ordering::Acquire)
            });
        if !valid {
            return;
        }
        if let Some(ready) = state.inflight.remove(&completion.input.ticket_id) {
            // Retain the authoritative result rather than trusting the returned payload.
            state.ready.push_front(ready);
        }
    }
    /// Completes callback delivery. A timed-out OS call retains its actual worker permit.
    pub(crate) fn finish_completion(&self, plugin: &str, generation: &str, ticket: &str) {
        let mut state = self.inner.state.lock().unwrap();
        if state
            .generations
            .get(plugin)
            .is_none_or(|v| v.id != generation)
        {
            return;
        }
        let Some(ready) = state.inflight.remove(ticket) else {
            return;
        };
        if ready.completion.plugin_id != plugin || ready.completion.generation != generation {
            state.inflight.insert(ticket.into(), ready);
            return;
        }
        let live = state.generations.get_mut(plugin).unwrap();
        let request_id = ready.completion.input.request_id.clone();
        if let Some(mut prior) = live.pending.remove(&request_id) {
            remember(
                live,
                prior.destination.clone(),
                prior.request.clone(),
                prior.ticket.clone(),
                ready.completion.input,
            );
            if prior.timed_out && !prior.worker_done {
                prior.delivery_finished = true;
                live.pending.insert(request_id.clone(), prior);
            }
        }
    }
    #[cfg(test)]
    fn pop_completion(&self) -> Option<StorageCompletion> {
        let identities = self
            .inner
            .state
            .lock()
            .unwrap()
            .generations
            .iter()
            .map(|(id, v)| (id.clone(), v.id.clone()))
            .collect::<Vec<_>>();
        for (id, generation) in identities {
            if let Some(completion) = self.pop_completion_for(&id, &generation) {
                self.finish_completion(&id, &generation, &completion.input.ticket_id);
                return Some(completion);
            }
        }
        None
    }
}
impl Drop for Storage {
    fn drop(&mut self) {
        let mut state = self.inner.state.lock().unwrap();
        for live in state.generations.values() {
            live.lease.withdrawn.store(true, Ordering::Release);
        }
        let mut pool = pool().state.lock().unwrap();
        pool.queue
            .retain(|work| !work.job.lease.withdrawn.load(Ordering::Acquire));
        state.generations.clear();
        state.ready.clear();
        state.inflight.clear();
    }
}

fn remember(
    live: &mut Generation,
    destination: Option<String>,
    request: StorageRequest,
    ticket: StorageTicket,
    input: StorageCompletionInput,
) {
    live.recent
        .retain(|v| v.ticket.request_id != ticket.request_id);
    live.recent.push_back(Recent {
        destination,
        request,
        ticket,
        input,
    });
    while live.recent.len() > 128 {
        live.recent.pop_front();
    }
}
fn timeout_result() -> HostResult<StorageCompletionValue> {
    HostResult::Err(HostFailure {
        ok: false,
        error: error(
            PLUGIN_TIMEOUT,
            "storage ticket expired; read before retrying",
        ),
    })
}

struct Work {
    owner: Weak<Inner>,
    job: Job,
}
#[derive(Default)]
struct PoolState {
    queue: VecDeque<Work>,
    active: BTreeSet<String>,
}
struct Pool {
    state: Mutex<PoolState>,
    wake: Condvar,
}
fn pool() -> &'static Arc<Pool> {
    static POOL: OnceLock<Arc<Pool>> = OnceLock::new();
    POOL.get_or_init(|| {
        let pool = Arc::new(Pool {
            state: Mutex::new(PoolState::default()),
            wake: Condvar::new(),
        });
        for index in 0..2 {
            let pool = pool.clone();
            std::thread::Builder::new()
                .name(format!("plugin-storage-{index}"))
                .spawn(move || loop {
                    let work = {
                        let mut state = pool.state.lock().unwrap();
                        loop {
                            if let Some(index) = state
                                .queue
                                .iter()
                                .position(|work| !state.active.contains(&work.job.plugin))
                            {
                                let work = state.queue.remove(index).unwrap();
                                state.active.insert(work.job.plugin.clone());
                                break work;
                            }
                            state = pool.wake.wait(state).unwrap();
                        }
                    };
                    let plugin = work.job.plugin.clone();
                    if let Some(owner) = work.owner.upgrade() {
                        run(owner, work.job);
                    }
                    pool.state.lock().unwrap().active.remove(&plugin);
                    pool.wake.notify_all();
                })
                .expect("start bounded storage workers");
        }
        pool
    })
}
fn schedule(inner: &Arc<Inner>, state: &mut State) {
    for (plugin, live) in &mut state.generations {
        if !state.busy.contains(plugin) && live.lease.active.load(Ordering::Acquire) {
            if let Some(job) = live.queue.pop_front() {
                state.busy.insert(plugin.clone());
                pool().state.lock().unwrap().queue.push_back(Work {
                    owner: Arc::downgrade(inner),
                    job,
                });
                pool().wake.notify_one();
            }
        }
    }
}
fn run(inner: Arc<Inner>, job: Job) {
    let result = if Instant::now() >= job.deadline {
        Err(error(
            PLUGIN_TIMEOUT,
            "storage ticket expired; read before retrying",
        ))
    } else {
        execute(&inner, &job)
    };
    let mut state = inner.state.lock().unwrap();
    state.busy.remove(&job.plugin);
    if !job.lease.withdrawn.load(Ordering::Acquire)
        && state
            .generations
            .get(&job.plugin)
            .is_some_and(|v| Arc::ptr_eq(&v.lease, &job.lease))
    {
        let live = state.generations.get_mut(&job.plugin).unwrap();
        if live
            .pending
            .get(&job.ticket.request_id)
            .is_some_and(|v| v.abandoned)
        {
            live.pending.remove(&job.ticket.request_id);
            drop(job._permit);
        } else if live
            .pending
            .get(&job.ticket.request_id)
            .is_some_and(|v| v.timed_out)
        {
            let prior = live.pending.get_mut(&job.ticket.request_id).unwrap();
            prior.worker_done = true;
            if prior.delivery_finished {
                live.pending.remove(&job.ticket.request_id);
            } else if let Some(ready) = state.inflight.get_mut(&job.ticket.ticket_id) {
                ready._permit = Some(job._permit);
            } else if let Some(ready) = state
                .ready
                .iter_mut()
                .find(|v| v.completion.input.ticket_id == job.ticket.ticket_id)
            {
                ready._permit = Some(job._permit);
            }
        } else {
            if let Some(prior) = live.pending.get_mut(&job.ticket.request_id) {
                prior.worker_done = true;
            }
            let result = match result {
                Ok(value) => HostResult::Ok(HostSuccess { ok: true, value }),
                Err(error) => HostResult::Err(HostFailure { ok: false, error }),
            };
            state.ready.push_back(Ready {
                completion: StorageCompletion {
                    plugin_id: job.plugin,
                    generation: job.generation,
                    input: StorageCompletionInput {
                        request_id: job.ticket.request_id,
                        ticket_id: job.ticket.ticket_id,
                        result,
                    },
                },
                lease: job.lease,
                _permit: Some(job._permit),
            });
        }
    }
    schedule(&inner, &mut state);
}

/// Cancellation checks the durable layer's last pre-publication boundary. Publication
/// commits even if subsequent namespace sync fails. Already-started IO may outlive revoke.
struct PublicationGuard {
    lease: Arc<Lease>,
    #[cfg(test)]
    fault: Option<CrashPoint>,
    #[cfg(test)]
    withdraw_at: Option<CrashPoint>,
}
impl CrashInjector for PublicationGuard {
    fn should_interrupt(&self, point: CrashPoint) -> bool {
        #[cfg(test)]
        if self.withdraw_at == Some(point) {
            self.lease.withdrawn.store(true, Ordering::Release);
        }
        if matches!(
            point,
            CrashPoint::BeforeTempSync | CrashPoint::AfterTempSync
        ) && self.lease.withdrawn.load(Ordering::Acquire)
        {
            return true;
        }
        #[cfg(test)]
        if self.fault == Some(point) {
            return true;
        }
        false
    }
}
#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Namespace {
    revision: u64,
    values: BTreeMap<String, Value>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Document {
    schema_version: u32,
    namespaces: BTreeMap<String, Namespace>,
}
impl Default for Document {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            namespaces: BTreeMap::new(),
        }
    }
}
impl Document {
    fn validate(&self) -> Result<()> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(corrupt());
        }
        if self.namespaces.len() > 256 {
            return Err(error(
                PLUGIN_QUOTA_EXCEEDED,
                "storage namespace quota exceeded",
            ));
        }
        let mut count = 0;
        for (name, namespace) in &self.namespaces {
            let project = if name == "profile" {
                None
            } else {
                Some(name.strip_prefix("project:").ok_or_else(corrupt)?)
            };
            if let Some(project_id) = project {
                StorageRequest::Get(StorageGetRequest {
                    request_id: "validate".into(),
                    scope: StorageScope::Project,
                    key: "validate".into(),
                    project_id: Some(project_id.into()),
                })
                .validate()
                .map_err(|_| corrupt())?;
            }
            if namespace.revision > MAX_SAFE_INTEGER {
                return Err(corrupt());
            }
            for (key, value) in &namespace.values {
                validate_storage_set_request(&StorageSetRequest {
                    request_id: "validate".into(),
                    scope: if project.is_some() {
                        StorageScope::Project
                    } else {
                        StorageScope::Profile
                    },
                    key: key.clone(),
                    project_id: project.map(str::to_owned),
                    value: value.clone(),
                    expected_revision: namespace.revision,
                })
                .map_err(|_| corrupt())?;
                count += 1;
            }
        }
        if count > MAX_KEYS {
            return Err(error(PLUGIN_QUOTA_EXCEEDED, "storage key quota exceeded"));
        }
        if serde_json::to_vec(self).map_err(|_| corrupt())?.len() > MAX_PLUGIN_BYTES {
            return Err(error(PLUGIN_QUOTA_EXCEEDED, "storage byte quota exceeded"));
        }
        Ok(())
    }
}
fn execute(inner: &Inner, job: &Job) -> Result<StorageCompletionValue> {
    if job.lease.withdrawn.load(Ordering::Acquire) {
        return Err(stale());
    }
    let fs = DurableFileSystem::with_crash_injector(Arc::new(PublicationGuard {
        lease: job.lease.clone(),
        #[cfg(test)]
        fault: inner.fault.lock().unwrap().take(),
        #[cfg(test)]
        withdraw_at: inner.withdraw_at.lock().unwrap().take(),
    }));
    fs.create_dir_durable(&inner.root, DirectoryPermissions::PrivateOwnerOnly)
        .map_err(|_| error(PLUGIN_IO, "storage directory unavailable"))?;
    owner_only_directory(&inner.root)?;
    let path = inner.root.join(format!("{}.json", job.plugin));
    let mut document = match read_optional(&path).map_err(|_| corrupt())? {
        Some(bytes) if bytes.len() <= MAX_PLUGIN_BYTES => {
            serde_json::from_slice::<Document>(&bytes).map_err(|_| corrupt())?
        }
        Some(_) => return Err(corrupt()),
        None => Document::default(),
    };
    document.validate().map_err(|_| corrupt())?;
    let (_, key, project) = job.request.identity();
    let name = project
        .map(|id| format!("project:{id}"))
        .unwrap_or_else(|| "profile".into());
    let namespace = document.namespaces.entry(name).or_default();
    let expected = match &job.request {
        StorageRequest::Get(_) => {
            return Ok(StorageCompletionValue::Get(
                match namespace.values.get(key) {
                    Some(value) => StorageGetValue::Found(StorageFoundValue {
                        found: true,
                        value: value.clone(),
                        revision: namespace.revision,
                    }),
                    None => StorageGetValue::Missing(StorageMissingValue {
                        found: false,
                        revision: namespace.revision,
                    }),
                },
            ))
        }
        StorageRequest::Set(v) => v.expected_revision,
        StorageRequest::Delete(v) => v.expected_revision,
    };
    if namespace.revision != expected {
        return Err(error(
            PLUGIN_STORAGE_CONFLICT,
            "storage namespace revision changed",
        ));
    }
    if matches!(&job.request, StorageRequest::Delete(_)) && !namespace.values.contains_key(key) {
        return Ok(StorageCompletionValue::Mutation(StorageMutationValue {
            revision: namespace.revision,
        }));
    }
    let revision = namespace
        .revision
        .checked_add(1)
        .filter(|v| *v <= MAX_SAFE_INTEGER)
        .ok_or_else(|| error(PLUGIN_IO, "storage namespace revision exhausted"))?;
    match &job.request {
        StorageRequest::Set(v) => {
            namespace.values.insert(key.into(), v.value.clone());
        }
        StorageRequest::Delete(_) => {
            namespace.values.remove(key);
        }
        StorageRequest::Get(_) => unreachable!(),
    }
    namespace.revision = revision;
    document.validate()?;
    let bytes = serde_json::to_vec(&document)
        .map_err(|_| error(PLUGIN_IO, "storage serialization failed"))?;
    if job.lease.withdrawn.load(Ordering::Acquire) {
        return Err(stale());
    }
    reject_non_regular(&path)?;
    finish_persist(&path, fs.replace_bytes(&path, &bytes))?;
    Ok(StorageCompletionValue::Mutation(StorageMutationValue {
        revision,
    }))
}
fn error(code: &str, message: &str) -> PluginError {
    PluginError::new(code, message)
}
fn stale() -> PluginError {
    error(PLUGIN_STALE_GENERATION, "storage generation withdrawn")
}
fn corrupt() -> PluginError {
    error(
        PLUGIN_STORE_CORRUPT,
        "plugin storage is corrupt or unsupported",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    const PLUGIN: &str = "dev.storage";
    static TEST_LOCK: Mutex<()> = Mutex::new(());
    fn fixture() -> (tempfile::TempDir, Storage, String) {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::new(dir.path().to_owned());
        let generation = Uuid::new_v4().to_string();
        storage.register(PLUGIN, &generation).unwrap();
        storage.activate(PLUGIN, &generation).unwrap();
        (dir, storage, generation)
    }
    fn set(key: &str, project: Option<&str>, revision: u64, value: Value) -> StorageRequest {
        StorageRequest::Set(StorageSetRequest {
            request_id: Uuid::new_v4().to_string(),
            scope: if project.is_some() {
                StorageScope::Project
            } else {
                StorageScope::Profile
            },
            key: key.into(),
            project_id: project.map(str::to_owned),
            expected_revision: revision,
            value,
        })
    }
    fn get(key: &str, project: Option<&str>) -> StorageRequest {
        StorageRequest::Get(StorageGetRequest {
            request_id: Uuid::new_v4().to_string(),
            scope: if project.is_some() {
                StorageScope::Project
            } else {
                StorageScope::Profile
            },
            key: key.into(),
            project_id: project.map(str::to_owned),
        })
    }
    fn delete(key: &str, revision: u64) -> StorageRequest {
        StorageRequest::Delete(StorageDeleteRequest {
            request_id: Uuid::new_v4().to_string(),
            scope: StorageScope::Profile,
            key: key.into(),
            project_id: None,
            expected_revision: revision,
        })
    }
    fn wait(storage: &Storage) -> StorageCompletion {
        let end = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(value) = storage.pop_completion() {
                validate_storage_completion_input(&value.input).unwrap();
                return value;
            }
            assert!(Instant::now() < end, "storage completion timed out");
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    fn request(
        storage: &Storage,
        generation: &str,
        request: StorageRequest,
    ) -> Result<StorageCompletionValue> {
        storage
            .inner
            .state
            .lock()
            .unwrap()
            .generations
            .get_mut(PLUGIN)
            .unwrap()
            .tokens = 20.0;
        let ticket = storage.accept(PLUGIN, generation, request)?;
        let completion = wait(storage);
        assert_eq!(completion.generation, generation);
        assert_eq!(completion.input.ticket_id, ticket.ticket_id);
        match completion.input.result {
            HostResult::Ok(ok) => Ok(ok.value),
            HostResult::Err(err) => Err(err.error),
        }
    }
    fn revision(value: StorageCompletionValue) -> u64 {
        match value {
            StorageCompletionValue::Mutation(v) => v.revision,
            _ => panic!("not mutation"),
        }
    }
    // Keep the generation unpublished and drive its owned jobs directly: no wall-clock
    // races, and no completion RPC is needed to clean up an abandoned worker.
    fn run_queued(storage: &Storage) {
        loop {
            let job = storage
                .inner
                .state
                .lock()
                .unwrap()
                .generations
                .get_mut(PLUGIN)
                .unwrap()
                .queue
                .pop_front();
            let Some(job) = job else { break };
            run(storage.inner.clone(), job);
        }
    }

    #[test]
    fn abandoned_web_workers_release_all_eight_slots_without_completion_rpc() {
        let _serial = TEST_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::new(dir.path().into());
        let generation = Uuid::new_v4().to_string();
        storage.register(PLUGIN, &generation).unwrap();
        // More than the global quota in total proves permits do not accumulate.
        for batch in 0..10 {
            storage
                .inner
                .state
                .lock()
                .unwrap()
                .generations
                .get_mut(PLUGIN)
                .unwrap()
                .tokens = 20.0;
            for slot in 0..8 {
                let request = if slot == 0 {
                    set("mutation", None, batch, serde_json::json!(batch))
                } else {
                    get(&format!("batch-{batch}-slot-{slot}"), None)
                };
                storage
                    .accept_destination(PLUGIN, &generation, request, Some("web:abandoned"))
                    .unwrap();
            }
            // Remove one job from the queue to model worker ownership before abandonment.
            let worker = storage
                .inner
                .state
                .lock()
                .unwrap()
                .generations
                .get_mut(PLUGIN)
                .unwrap()
                .queue
                .pop_front()
                .unwrap();
            storage.abandon_destination(PLUGIN, &generation, "web:abandoned");
            assert_eq!(storage.pending_count(), 8);
            assert_eq!(
                storage
                    .accept(PLUGIN, &generation, get("overflow", None))
                    .unwrap_err()
                    .code,
                PLUGIN_BUSY
            );
            assert!(storage.pop_completion_for(PLUGIN, &generation).is_none());
            assert!(storage
                .pop_completion_destination(PLUGIN, &generation, Some("web:abandoned"))
                .is_none());
            run(storage.inner.clone(), worker);
            run_queued(&storage);
            assert_eq!(storage.pending_count(), 0);
            let state = storage.inner.state.lock().unwrap();
            assert!(state.ready.is_empty());
            assert!(state.inflight.is_empty());
        }
        // Native completion delivery remains unchanged after abandoned web work.
        storage
            .accept(PLUGIN, &generation, get("mutation", None))
            .unwrap();
        run_queued(&storage);
        storage.activate(PLUGIN, &generation).unwrap();
        let completion = storage.pop_completion_for(PLUGIN, &generation).unwrap();
        assert!(
            matches!(&completion.input.result, HostResult::Ok(HostSuccess {
            value: StorageCompletionValue::Get(StorageGetValue::Found(StorageFoundValue {
                revision: 10, value, ..
            })), ..
        }) if *value == serde_json::json!(9))
        );
        storage.finish_completion(PLUGIN, &generation, &completion.input.ticket_id);
        assert_eq!(storage.pending_count(), 0);
    }

    #[test]
    fn abandoned_web_destinations_discard_ready_inflight_cached_and_timeout_results() {
        let _serial = TEST_LOCK.lock().unwrap();
        for phase in ["ready", "inflight", "cached", "timeout"] {
            let dir = tempfile::tempdir().unwrap();
            let storage = Storage::new(dir.path().into());
            let generation = Uuid::new_v4().to_string();
            storage.register(PLUGIN, &generation).unwrap();
            let request = get("key", None);
            storage
                .accept_destination(PLUGIN, &generation, request.clone(), Some("web:request"))
                .unwrap();
            if phase == "timeout" {
                storage
                    .inner
                    .state
                    .lock()
                    .unwrap()
                    .generations
                    .get_mut(PLUGIN)
                    .unwrap()
                    .pending
                    .values_mut()
                    .next()
                    .unwrap()
                    .deadline = Instant::now();
            } else {
                run_queued(&storage);
            }
            if phase != "ready" {
                // Enable delivery without scheduling the deliberately parked timeout job.
                storage
                    .inner
                    .state
                    .lock()
                    .unwrap()
                    .generations
                    .get(PLUGIN)
                    .unwrap()
                    .lease
                    .active
                    .store(true, Ordering::Release);
                let completion = storage
                    .pop_completion_destination(PLUGIN, &generation, Some("web:request"))
                    .unwrap();
                if phase == "cached" {
                    storage.finish_completion(PLUGIN, &generation, &completion.input.ticket_id);
                    storage
                        .accept_destination(PLUGIN, &generation, request, Some("web:request"))
                        .unwrap();
                }
            }
            storage.abandon_destination(PLUGIN, &generation, "web:request");
            if phase == "timeout" {
                // A synthetic timeout must not free an unfinished filesystem worker.
                assert_eq!(storage.pending_count(), 1);
                run_queued(&storage);
            }
            assert_eq!(storage.pending_count(), 0, "{phase}");
            assert!(storage.pop_completion_for(PLUGIN, &generation).is_none());
            let state = storage.inner.state.lock().unwrap();
            assert!(state.ready.is_empty(), "{phase}");
            assert!(state.inflight.is_empty(), "{phase}");
        }
    }

    #[test]
    fn storage_project_ids_are_opaque_and_tombstones_validate() {
        let _serial = TEST_LOCK.lock().unwrap();
        let (_dir, storage, generation) = fixture();
        assert_eq!(
            revision(
                request(
                    &storage,
                    &generation,
                    set("key", Some("p1"), 0, Value::Null)
                )
                .unwrap()
            ),
            1
        );
        assert!(matches!(
            request(&storage, &generation, get("key", Some("p1"))).unwrap(),
            StorageCompletionValue::Get(StorageGetValue::Found(StorageFoundValue {
                found: true,
                value: Value::Null,
                revision: 1
            }))
        ));
        assert_eq!(
            revision(
                request(
                    &storage,
                    &generation,
                    StorageRequest::Delete(StorageDeleteRequest {
                        request_id: "delete-project".into(),
                        scope: StorageScope::Project,
                        key: "key".into(),
                        project_id: Some("p1".into()),
                        expected_revision: 1,
                    })
                )
                .unwrap()
            ),
            2
        );
        assert!(matches!(
            request(&storage, &generation, get("key", Some("p1"))).unwrap(),
            StorageCompletionValue::Get(StorageGetValue::Missing(StorageMissingValue {
                found: false,
                revision: 2
            }))
        ));
        let invalid: Document =
            serde_json::from_value(serde_json::json!({"schemaVersion":1,"namespaces":{
            "project:":{"revision":0,"values":{}}}}))
            .unwrap();
        assert_eq!(invalid.validate().unwrap_err().code, PLUGIN_STORE_CORRUPT);
    }
    #[test]
    fn storage_namespace_cas_and_quotas() {
        let _serial = TEST_LOCK.lock().unwrap();
        let (_dir, storage, generation) = fixture();
        let project = Uuid::new_v4().to_string();
        assert_eq!(
            revision(request(&storage, &generation, delete("missing", 0)).unwrap()),
            0
        );
        assert_eq!(
            revision(request(&storage, &generation, set("same", None, 0, Value::Null)).unwrap()),
            1
        );
        assert_eq!(
            request(&storage, &generation, set("other", None, 0, Value::Null))
                .unwrap_err()
                .code,
            PLUGIN_STORAGE_CONFLICT
        );
        assert_eq!(
            revision(
                request(
                    &storage,
                    &generation,
                    set("same", Some(&project), 0, Value::Bool(true))
                )
                .unwrap()
            ),
            1
        );
        assert!(matches!(
            request(&storage, &generation, get("same", None)).unwrap(),
            StorageCompletionValue::Get(StorageGetValue::Found(StorageFoundValue {
                value: Value::Null,
                revision: 1,
                ..
            }))
        ));
        assert!(matches!(
            request(
                &storage,
                &generation,
                get("same", Some(&Uuid::new_v4().to_string()))
            )
            .unwrap(),
            StorageCompletionValue::Get(StorageGetValue::Missing(StorageMissingValue {
                revision: 0,
                ..
            }))
        ));
        assert!(storage
            .accept(
                PLUGIN,
                &generation,
                set(
                    "large",
                    None,
                    1,
                    Value::String("a".repeat(MAX_STORAGE_VALUE_BYTES))
                )
            )
            .is_err());
        for i in 0..254 {
            request(
                &storage,
                &generation,
                set(&format!("k{i}"), None, i + 1, Value::Null),
            )
            .unwrap();
        }
        assert_eq!(
            request(
                &storage,
                &generation,
                set("overflow", Some(&project), 1, Value::Null)
            )
            .unwrap_err()
            .code,
            PLUGIN_QUOTA_EXCEEDED
        );
        assert_eq!(
            revision(request(&storage, &generation, delete("same", 255)).unwrap()),
            256
        );
        assert_eq!(
            revision(
                request(
                    &storage,
                    &generation,
                    set("replacement", None, 256, Value::Null)
                )
                .unwrap()
            ),
            257
        );
        let (_dir2, storage2, gen2) = fixture();
        for i in 0..15 {
            request(
                &storage2,
                &gen2,
                set(&format!("v{i}"), None, i, Value::String("x".repeat(65534))),
            )
            .unwrap();
        }
        assert_eq!(
            request(
                &storage2,
                &gen2,
                set("overflow", None, 15, Value::String("x".repeat(65534)))
            )
            .unwrap_err()
            .code,
            PLUGIN_QUOTA_EXCEEDED
        );
    }
    #[test]
    fn storage_publication_is_commit_point() {
        let _serial = TEST_LOCK.lock().unwrap();
        let (dir, storage, generation) = fixture();
        request(
            &storage,
            &generation,
            set("key", None, 0, Value::Bool(false)),
        )
        .unwrap();
        let path = dir.path().join("plugins/data/dev.storage.json");
        let original = std::fs::read(&path).unwrap();
        *storage.inner.fault.lock().unwrap() = Some(CrashPoint::AfterTempSync);
        assert_eq!(
            request(
                &storage,
                &generation,
                set("key", None, 1, Value::Bool(true))
            )
            .unwrap_err()
            .code,
            PLUGIN_IO
        );
        assert_eq!(std::fs::read(&path).unwrap(), original);
        *storage.inner.fault.lock().unwrap() = Some(CrashPoint::AfterReplace);
        assert_eq!(
            revision(
                request(
                    &storage,
                    &generation,
                    set("key", None, 1, Value::Bool(true))
                )
                .unwrap()
            ),
            2
        );
        let committed: Document = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(committed.namespaces["profile"].revision, 2);
        assert_eq!(
            committed.namespaces["profile"].values["key"],
            Value::Bool(true)
        );
        *storage.inner.fault.lock().unwrap() = Some(CrashPoint::AfterNamespaceSync);
        assert_eq!(
            revision(request(&storage, &generation, set("key", None, 2, Value::Null)).unwrap()),
            3
        );
        storage.revoke(PLUGIN, &generation);
        assert!(path.exists(), "withdraw/uninstall must retain storage");
        let next = Uuid::new_v4().to_string();
        storage.register(PLUGIN, &next).unwrap();
        storage.activate(PLUGIN, &next).unwrap();
        assert!(matches!(
            request(&storage, &next, get("key", None)).unwrap(),
            StorageCompletionValue::Get(StorageGetValue::Found(StorageFoundValue {
                revision: 3,
                value: Value::Null,
                ..
            }))
        ));
    }
    #[test]
    fn storage_completion_retry_retains_slots_until_acknowledged() {
        let _serial = TEST_LOCK.lock().unwrap();
        let (_dir, storage, generation) = fixture();
        let write = set("key", None, 0, Value::Null);
        let ticket = storage.accept(PLUGIN, &generation, write.clone()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let first = loop {
            if let Some(value) = storage.pop_completion_for(PLUGIN, &generation) {
                break value;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        };
        assert_eq!(storage.pending_count(), 1);
        assert!(storage.pop_completion_for(PLUGIN, &generation).is_none());
        assert_eq!(storage.accept(PLUGIN, &generation, write).unwrap(), ticket);
        storage.return_completion(first);
        let retry = storage.pop_completion_for(PLUGIN, &generation).unwrap();
        assert_eq!(retry.input.ticket_id, ticket.ticket_id);
        assert_eq!(storage.pending_count(), 1);
        storage.finish_completion(PLUGIN, &generation, &ticket.ticket_id);
        assert_eq!(storage.pending_count(), 0);
        storage.return_completion(retry);
        assert!(storage.pop_completion_for(PLUGIN, &generation).is_none());
        storage.revoke(PLUGIN, &generation);
        assert!(storage.pop_completion_for(PLUGIN, &generation).is_none());
    }
    #[test]
    fn storage_global_admission_is_bounded_and_revocation_releases_slots() {
        let _serial = TEST_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::new(dir.path().to_owned());
        let generation = Uuid::new_v4().to_string();
        let mut accepted = 0;
        let mut busy = 0;
        for index in 0..9 {
            let plugin = format!("dev.global{index}");
            storage.register(&plugin, &generation).unwrap();
            for _ in 0..MAX_GENERATION_PENDING {
                match storage.accept(&plugin, &generation, get("key", None)) {
                    Ok(_) => accepted += 1,
                    Err(error) => {
                        assert_eq!(error.code, PLUGIN_BUSY);
                        busy += 1;
                    }
                }
            }
        }
        assert!(accepted <= MAX_GLOBAL_PENDING);
        assert!(busy >= MAX_GENERATION_PENDING);
        assert_eq!(storage.pending_count(), accepted);
        for index in 0..9 {
            storage.revoke(&format!("dev.global{index}"), &generation);
        }
        assert_eq!(storage.pending_count(), 0);
        assert!(storage.pop_completion().is_none());
        assert!(!dir.path().join("plugins").exists());
        assert!(pool().state.lock().unwrap().active.len() <= 2);
    }
    #[cfg(unix)]
    #[test]
    fn storage_symlink_and_namespace_tombstone_limits_preserve_files() {
        let _serial = TEST_LOCK.lock().unwrap();
        let (dir, storage, generation) = fixture();
        let root = dir.path().join("plugins/data");
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("dev.storage.json");
        let target = dir.path().join("untouched");
        std::fs::write(&target, b"sensitive").unwrap();
        std::os::unix::fs::symlink(&target, &path).unwrap();
        assert_eq!(
            request(&storage, &generation, set("key", None, 0, Value::Null))
                .unwrap_err()
                .code,
            PLUGIN_STORE_CORRUPT
        );
        assert_eq!(std::fs::read(&target).unwrap(), b"sensitive");
        std::fs::remove_file(&path).unwrap();
        let mut document = Document::default();
        for _ in 0..256 {
            document
                .namespaces
                .insert(format!("project:{}", Uuid::new_v4()), Namespace::default());
        }
        let original = serde_json::to_vec(&document).unwrap();
        std::fs::write(&path, &original).unwrap();
        assert_eq!(
            request(&storage, &generation, set("key", None, 0, Value::Null))
                .unwrap_err()
                .code,
            PLUGIN_QUOTA_EXCEEDED
        );
        assert_eq!(std::fs::read(&path).unwrap(), original);
        std::fs::remove_file(&path).unwrap();
        request(&storage, &generation, set("key", None, 0, Value::Null)).unwrap();
        request(&storage, &generation, delete("key", 1)).unwrap();
        assert!(matches!(
            request(&storage, &generation, get("key", None)).unwrap(),
            StorageCompletionValue::Get(StorageGetValue::Missing(StorageMissingValue {
                revision: 2,
                ..
            }))
        ));
        assert_eq!(
            revision(request(&storage, &generation, delete("key", 2)).unwrap()),
            2
        );
        let retained: Document = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(retained.namespaces["profile"].revision, 2);
    }
    #[test]
    fn storage_idempotency_replays_without_advancing_revision() {
        let _serial = TEST_LOCK.lock().unwrap();
        let (_dir, storage, generation) = fixture();
        let write = set("key", None, 0, Value::Bool(true));
        let ticket = storage.accept(PLUGIN, &generation, write.clone()).unwrap();
        let first = wait(&storage);
        assert_eq!(
            storage.accept(PLUGIN, &generation, write.clone()).unwrap(),
            ticket
        );
        let replay = wait(&storage);
        assert_eq!(first.input, replay.input);
        let mut changed = write;
        if let StorageRequest::Set(v) = &mut changed {
            v.value = Value::Null;
        }
        assert_eq!(
            storage
                .accept(PLUGIN, &generation, changed)
                .unwrap_err()
                .code,
            PLUGIN_INVALID_REQUEST
        );
        assert_eq!(
            revision(request(&storage, &generation, set("another", None, 1, Value::Null)).unwrap()),
            2
        );
        storage
            .inner
            .state
            .lock()
            .unwrap()
            .generations
            .get_mut(PLUGIN)
            .unwrap()
            .tokens = 0.0;
        storage
            .inner
            .state
            .lock()
            .unwrap()
            .generations
            .get_mut(PLUGIN)
            .unwrap()
            .refill = Instant::now();
        assert_eq!(
            storage
                .accept(PLUGIN, &generation, get("key", None))
                .unwrap_err()
                .code,
            PLUGIN_BUSY
        );
    }
    #[test]
    fn storage_withdrawal_at_publication_boundaries_never_delivers_callback() {
        let _serial = TEST_LOCK.lock().unwrap();
        let (dir, storage, generation) = fixture();
        request(
            &storage,
            &generation,
            set("key", None, 0, Value::Bool(false)),
        )
        .unwrap();
        let path = dir.path().join("plugins/data/dev.storage.json");
        for (point, expected_revision) in [
            (CrashPoint::AfterTempSync, 1),
            (CrashPoint::AfterReplace, 2),
        ] {
            let current = storage.inner.state.lock().unwrap().generations[PLUGIN]
                .id
                .clone();
            *storage.inner.withdraw_at.lock().unwrap() = Some(point);
            storage
                .accept(PLUGIN, &current, set("key", None, 1, Value::Bool(true)))
                .unwrap();
            let deadline = Instant::now() + Duration::from_secs(10);
            while storage.inner.state.lock().unwrap().busy.contains(PLUGIN) {
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(1));
            }
            assert!(storage.pop_completion().is_none());
            let document: Document =
                serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            assert_eq!(document.namespaces["profile"].revision, expected_revision);
            storage.revoke(PLUGIN, &current);
            let next = Uuid::new_v4().to_string();
            storage.register(PLUGIN, &next).unwrap();
            storage.activate(PLUGIN, &next).unwrap();
        }
    }
    #[test]
    fn storage_expired_queued_ticket_and_corrupt_file_fail_without_mutation() {
        let _serial = TEST_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::new(dir.path().to_owned());
        let generation = Uuid::new_v4().to_string();
        storage.register(PLUGIN, &generation).unwrap();
        let ticket = storage
            .accept(PLUGIN, &generation, set("key", None, 0, Value::Null))
            .unwrap();
        {
            let mut state = storage.inner.state.lock().unwrap();
            let live = state.generations.get_mut(PLUGIN).unwrap();
            let expired = Instant::now() - Duration::from_secs(1);
            live.queue.front_mut().unwrap().deadline = expired;
            live.pending.get_mut(&ticket.request_id).unwrap().deadline = expired;
        }
        storage.activate(PLUGIN, &generation).unwrap();
        assert!(
            matches!(wait(&storage).input.result, HostResult::Err(HostFailure { error: PluginError { code, .. }, .. }) if code == PLUGIN_TIMEOUT)
        );
        assert!(!dir.path().join("plugins/data/dev.storage.json").exists());
        let root = dir.path().join("plugins/data");
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("dev.storage.json");
        std::fs::write(&path, b"not-json").unwrap();
        assert_eq!(
            request(&storage, &generation, set("key", None, 0, Value::Null))
                .unwrap_err()
                .code,
            PLUGIN_STORE_CORRUPT
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"not-json");
    }
    #[test]
    fn storage_admission_is_io_free_and_withdrawal_cancels_create_queue() {
        let _serial = TEST_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("not-created");
        let storage = Storage::new(root.clone());
        let generation = Uuid::new_v4().to_string();
        storage.register(PLUGIN, &generation).unwrap();
        let first = set("key", None, 0, Value::Null);
        let ticket = storage.accept(PLUGIN, &generation, first.clone()).unwrap();
        assert_eq!(
            storage.accept(PLUGIN, &generation, first.clone()).unwrap(),
            ticket
        );
        let mut altered = first;
        if let StorageRequest::Set(v) = &mut altered {
            v.value = Value::Bool(true);
        }
        assert_eq!(
            storage
                .accept(PLUGIN, &generation, altered)
                .unwrap_err()
                .code,
            PLUGIN_INVALID_REQUEST
        );
        for i in 1..MAX_GENERATION_PENDING {
            storage
                .accept(
                    PLUGIN,
                    &generation,
                    set(&format!("k{i}"), None, i as u64, Value::Null),
                )
                .unwrap();
        }
        assert_eq!(
            storage
                .accept(PLUGIN, &generation, get("x", None))
                .unwrap_err()
                .code,
            PLUGIN_BUSY
        );
        assert!(!root.exists());
        storage.revoke(PLUGIN, &generation);
        assert!(storage.pop_completion().is_none());
        assert_eq!(
            storage.activate(PLUGIN, &generation).unwrap_err().code,
            PLUGIN_STALE_GENERATION
        );
        assert!(!root.exists());
    }
}

#[cfg(test)]
mod web_destination_tests {
    use super::*;
    #[tokio::test]
    async fn web_storage_completions_never_reach_native() {
        let profile = std::env::temp_dir().join(format!("web-storage-{}", Uuid::new_v4()));
        let storage = Storage::new(profile.clone());
        let plugin = "org.test.web";
        let generation = Uuid::new_v4().to_string();
        storage.register(plugin, &generation).unwrap();
        storage.activate(plugin, &generation).unwrap();
        let request=parse_storage_set_request(&serde_json::to_vec(&serde_json::json!({"requestId":Uuid::new_v4().to_string(),"scope":"profile","key":"state","value":{"n":1},"expectedRevision":0})).unwrap()).unwrap();
        storage
            .accept_destination(
                plugin,
                &generation,
                StorageRequest::Set(request),
                Some("web-session:request"),
            )
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            assert!(storage.pop_completion_for(plugin, &generation).is_none());
            if let Some(completion) =
                storage.pop_completion_destination(plugin, &generation, Some("web-session:request"))
            {
                assert!(matches!(completion.input.result, HostResult::Ok(_)));
                storage.finish_completion(plugin, &generation, &completion.input.ticket_id);
                break;
            }
            assert!(Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(storage.pop_completion_for(plugin, &generation).is_none());
        storage.revoke(plugin, &generation);
        let _ = std::fs::remove_dir_all(profile);
    }
}
