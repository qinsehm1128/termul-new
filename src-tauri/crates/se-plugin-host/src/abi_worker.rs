use crate::{
    broker::PluginBroker,
    loader::{self, Artifact},
    registry::Journal,
    runtime::{HostOptions, RestartReason},
    validation::{error, status},
};
use se_plugin_abi::*;
use se_plugin_protocol::*;
use std::{
    ffi::c_void,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::{self, SyncSender},
        Arc,
    },
    time::Instant,
};
use tokio::sync::watch;

#[derive(Default)]
pub(crate) struct Metrics {
    pub callbacks_in_flight: AtomicUsize,
    pub destroy_attempts: AtomicUsize,
    pub loaded_libraries: AtomicUsize,
    pub panics: std::sync::Mutex<Vec<RestartReason>>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum CallKind {
    Create,
    Render,
    Action,
    Command,
    Config,
    Service,
    Destroy,
}
impl CallKind {
    fn name(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Render => "render_view",
            Self::Action => "on_action",
            Self::Command => "on_command",
            Self::Config => "on_config_changed",
            Self::Service => "on_service_call",
            Self::Destroy => "destroy",
        }
    }
}
struct Request {
    kind: CallKind,
    input: Vec<u8>,
    deadline: Instant,
    chain: Vec<String>,
    reply: SyncSender<Result<Vec<u8>, PluginError>>,
    phase: Arc<AtomicUsize>,
}

pub(crate) struct AbiWorker {
    sender: SyncSender<Request>,
    poisoned: Arc<AtomicBool>,
    pub id: String,
    pub generation: String,
    options: HostOptions,
    restart: watch::Sender<Option<RestartReason>>,
    last_error: std::sync::Mutex<Option<PluginError>>,
    metrics: Arc<Metrics>,
    broker: std::sync::Weak<PluginBroker>,
    teardown_deadline: std::sync::Mutex<Option<Instant>>,
}

impl AbiWorker {
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn start(
        artifact: Artifact,
        id: String,
        generation: String,
        broker: Arc<PluginBroker>,
        journal: Arc<Journal>,
        options: HostOptions,
        metrics: Arc<Metrics>,
        restart: watch::Sender<Option<RestartReason>>,
    ) -> Result<(Arc<Self>, Manifest), PluginError> {
        Self::start_with(
            artifact,
            id,
            generation,
            broker,
            journal,
            options,
            metrics,
            restart,
            loader::load,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn start_with<F>(
        artifact: Artifact,
        id: String,
        generation: String,
        broker: Arc<PluginBroker>,
        journal: Arc<Journal>,
        options: HostOptions,
        metrics: Arc<Metrics>,
        restart: watch::Sender<Option<RestartReason>>,
        load: F,
    ) -> Result<(Arc<Self>, Manifest), PluginError>
    where
        F: FnOnce(&Artifact, &Arc<Metrics>) -> Result<(SePluginApiV1, Manifest), PluginError>
            + Send
            + 'static,
    {
        let (sender, receiver) = mpsc::sync_channel::<Request>(options.queue_capacity);
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let poisoned = Arc::new(AtomicBool::new(false));
        let worker = Arc::new(Self {
            sender,
            poisoned: poisoned.clone(),
            id: id.clone(),
            generation: generation.clone(),
            options: options.clone(),
            restart,
            last_error: std::sync::Mutex::new(None),
            metrics: metrics.clone(),
            broker: Arc::downgrade(&broker),
            teardown_deadline: std::sync::Mutex::new(None),
        });
        let thread_name = format!("plugin-{}", &generation[..generation.len().min(8)]);
        let fallback_restart = worker.restart.clone();
        let fallback_timeout = options.callback_timeout;
        std::thread::Builder::new()
            .name(thread_name)
            .spawn(move || {
                let token = match journal.begin(
                    if id.is_empty() {
                        None
                    } else {
                        Some(id.clone())
                    },
                    generation.clone(),
                    artifact.sha256.clone(),
                    "load".into(),
                ) {
                    Ok(token) => token,
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                        return;
                    }
                };
                metrics.callbacks_in_flight.fetch_add(1, Ordering::Relaxed);
                let loaded = load(&artifact, &metrics);
                metrics.callbacks_in_flight.fetch_sub(1, Ordering::Relaxed);
                let loaded = match journal.end(&token) {
                    Ok(()) => loaded,
                    Err(e) => Err(e),
                };
                let (api, manifest) = match loaded {
                    Ok(pair) => pair,
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                        return;
                    }
                };
                let mut context = Box::new(HostContext {
                    id: manifest.id.clone(),
                    generation: generation.clone(),
                    broker,
                    deadline: Instant::now(),
                    chain: Vec::new(),
                });
                let host = Box::new(SeHostApiV1 {
                    abi_version: SE_ABI_VERSION,
                    struct_size: std::mem::size_of::<SeHostApiV1>() as u32,
                    host_context: (&mut *context as *mut HostContext).cast(),
                    log: Some(host_log),
                    broker_call: Some(host_broker),
                });
                let mut state = WorkerState {
                    api,
                    instance: std::ptr::null_mut(),
                    context,
                    host,
                    metrics,
                    journal,
                    artifact,
                    destroyed: false,
                    output_bytes: vec![0; MAX_JSON_BYTES],
                    output: Box::new(SeOutput {
                        ptr: std::ptr::null_mut(),
                        capacity: 0,
                        len: 0,
                    }),
                };
                if ready_tx.send(Ok(manifest)).is_err() {
                    std::mem::forget(state);
                    return;
                }
                while let Ok(request) = receiver.recv() {
                    if poisoned.load(Ordering::Acquire) {
                        let _ = request.reply.send(Err(error(
                            PLUGIN_QUARANTINED,
                            "worker is quarantined until Core restart",
                        )));
                        continue;
                    }
                    if Instant::now() >= request.deadline {
                        let _ = request.reply.send(Err(error(
                            PLUGIN_TIMEOUT,
                            "queued native callback missed its deadline",
                        )));
                        continue;
                    }
                    if request
                        .phase
                        .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
                        .is_err()
                    {
                        continue;
                    }
                    state.context.deadline = request.deadline;
                    state.context.chain = request.chain;
                    let result = state.invoke(request.kind, &request.input);
                    if poisoned.load(Ordering::Acquire) {
                        std::mem::forget(request.input);
                        std::mem::forget(result);
                        continue;
                    }
                    if let Err(unsent) = request.reply.send(result) {
                        // The waiter can expire just after the native call returns.
                        if poisoned.load(Ordering::Acquire) {
                            std::mem::forget(request.input);
                            std::mem::forget(unsent.0);
                        }
                    }
                }
                if !state.instance.is_null() && !poisoned.load(Ordering::Acquire) {
                    let (done, wait) = mpsc::sync_channel(1);
                    let flag = poisoned.clone();
                    let reason = RestartReason {
                        plugin_id: Some(state.context.id.clone()),
                        generation: state.context.generation.clone(),
                        code: PLUGIN_TIMEOUT.into(),
                    };
                    let restart = fallback_restart.clone();
                    // Fallback teardown remains supervised even with no async caller.
                    let watchdog = std::thread::Builder::new()
                        .name("plugin-cleanup-watchdog".into())
                        .spawn(move || {
                            if wait.recv_timeout(fallback_timeout).is_err() {
                                flag.store(true, Ordering::Release);
                                restart.send_replace(Some(reason));
                            }
                        });
                    if watchdog.is_ok() {
                        let _ = state.invoke(CallKind::Destroy, &[]);
                        let _ = done.send(());
                    } else {
                        poisoned.store(true, Ordering::Release);
                        fallback_restart.send_replace(Some(RestartReason {
                            plugin_id: Some(state.context.id.clone()),
                            generation: state.context.generation.clone(),
                            code: PLUGIN_TIMEOUT.into(),
                        }));
                    }
                }
                // Timed out native state survives until process exit.
                if !state.instance.is_null() || poisoned.load(Ordering::Acquire) {
                    std::mem::forget(state);
                }
            })
            .map_err(crate::validation::io)?;
        let deadline = crate::validation::deadline(options.callback_timeout);
        let ready = tokio::task::spawn_blocking(move || {
            ready_rx.recv_timeout(deadline.saturating_duration_since(Instant::now()))
        })
        .await
        .map_err(crate::validation::io)?;
        match ready {
            Ok(Ok(manifest)) => Ok((worker, manifest)),
            Ok(Err(e)) => {
                worker.observe_failure(&e);
                Err(e)
            }
            Err(_) => {
                let e = error(PLUGIN_TIMEOUT, "native loader exceeded deadline");
                worker.observe_failure(&e);
                Err(e)
            }
        }
    }

    pub(crate) async fn call(
        self: &Arc<Self>,
        kind: CallKind,
        input: Vec<u8>,
    ) -> Result<Vec<u8>, PluginError> {
        let worker = self.clone();
        let mut deadline = crate::validation::deadline(self.options.callback_timeout);
        if matches!(kind, CallKind::Destroy) {
            if let Some(teardown) = *self
                .teardown_deadline
                .lock()
                .unwrap_or_else(|p| p.into_inner())
            {
                deadline = deadline.min(teardown);
            }
        }
        tokio::task::spawn_blocking(move || worker.call_blocking(kind, input, deadline, Vec::new()))
            .await
            .map_err(crate::validation::io)?
    }

    pub(crate) fn call_blocking(
        &self,
        kind: CallKind,
        input: Vec<u8>,
        deadline: Instant,
        chain: Vec<String>,
    ) -> Result<Vec<u8>, PluginError> {
        if !matches!(kind, CallKind::Destroy)
            && self.last_error().is_some_and(|e| e.code == PLUGIN_PANIC)
        {
            return Err(error(PLUGIN_QUARANTINED, "panicked plugin awaits teardown"));
        }
        if self.poisoned.load(Ordering::Acquire) {
            return Err(error(PLUGIN_QUARANTINED, "native worker is quarantined"));
        }
        let (reply, receiver) = mpsc::sync_channel(1);
        let phase = Arc::new(AtomicUsize::new(0));
        self.sender
            .try_send(Request {
                kind,
                input,
                deadline,
                chain,
                reply,
                phase: phase.clone(),
            })
            .map_err(|e| match e {
                mpsc::TrySendError::Full(_) => error(PLUGIN_BUSY, "native callback queue is full"),
                mpsc::TrySendError::Disconnected(_) => {
                    error(PLUGIN_QUARANTINED, "native worker is unavailable")
                }
            })?;
        let result = receiver
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .unwrap_or_else(|_| Err(error(PLUGIN_TIMEOUT, "native callback deadline exceeded")));
        if let Err(e) = &result {
            if e.code == PLUGIN_TIMEOUT
                && phase
                    .compare_exchange(0, 2, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
            {
                return Err(error(
                    PLUGIN_BUSY,
                    "callback expired before entering native code",
                ));
            }
            self.observe_failure(e);
        }
        result
    }

    pub(crate) fn set_teardown_deadline(&self, deadline: Instant) {
        *self
            .teardown_deadline
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = Some(deadline);
    }

    pub(crate) fn last_error(&self) -> Option<PluginError> {
        self.last_error
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    fn observe_failure(&self, failure: &PluginError) {
        *self.last_error.lock().unwrap_or_else(|p| p.into_inner()) = Some(failure.clone());
        if crate::validation::fatal(failure) {
            if let Some(broker) = self.broker.upgrade() {
                broker.withdraw(&self.id, &self.generation);
            }
        }
        if failure.code == PLUGIN_PANIC {
            self.metrics
                .panics
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push(RestartReason {
                    plugin_id: Some(self.id.clone()),
                    generation: self.generation.clone(),
                    code: failure.code.clone(),
                });
        }
        if failure.code == PLUGIN_TIMEOUT && !self.poisoned.swap(true, Ordering::AcqRel) {
            self.restart.send_replace(Some(RestartReason {
                plugin_id: if self.id.is_empty() {
                    None
                } else {
                    Some(self.id.clone())
                },
                generation: self.generation.clone(),
                code: failure.code.clone(),
            }));
        }
    }
}

struct HostContext {
    id: String,
    generation: String,
    broker: Arc<PluginBroker>,
    deadline: Instant,
    chain: Vec<String>,
}

unsafe extern "C" fn host_log(_context: *mut c_void, _level: u32, message: SeSlice) -> SeStatus {
    if message.len > 8192 || (message.len != 0 && message.ptr.is_null()) {
        return SE_INVALID_ARGUMENT;
    }
    // Do not emit plugin-supplied contents: they can contain configuration secrets.
    SE_OK
}
unsafe extern "C" fn host_broker(
    context: *mut c_void,
    input: SeSlice,
    output: *mut SeOutput,
) -> SeStatus {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if context.is_null()
            || input.ptr.is_null()
            || input.len > MAX_JSON_BYTES
            || output.is_null()
        {
            return SE_INVALID_ARGUMENT;
        }
        let context = unsafe { &*context.cast::<HostContext>() };
        let request = match parse_service_request(unsafe {
            std::slice::from_raw_parts(input.ptr, input.len)
        }) {
            Ok(value) => value,
            Err(_) => return SE_INVALID_JSON,
        };
        let bytes = match context.broker.call(
            &context.id,
            &context.generation,
            request,
            context.deadline,
            context.chain.clone(),
        ) {
            Ok(bytes) => bytes,
            Err(e) => {
                return match e.code.as_str() {
                    PLUGIN_TIMEOUT => SE_DEADLINE_EXCEEDED,
                    PLUGIN_STALE_GENERATION => SE_STALE_GENERATION,
                    PLUGIN_SERVICE_CYCLE | PLUGIN_REENTRANT => SE_REENTRANT,
                    PLUGIN_BUSY => SE_BUSY,
                    _ => SE_UNAVAILABLE,
                }
            }
        };
        let mut writer = match unsafe { OutputWriter::from_raw(output) } {
            Ok(value) => value,
            Err(e) => return e,
        };
        match writer.write(&bytes) {
            Ok(()) => SE_OK,
            Err(e) => e,
        }
    }));
    match result {
        Ok(status) => status,
        Err(payload) => {
            std::mem::forget(payload);
            SE_INTERNAL
        }
    }
}

struct WorkerState {
    api: SePluginApiV1,
    instance: *mut c_void,
    context: Box<HostContext>,
    host: Box<SeHostApiV1>,
    metrics: Arc<Metrics>,
    journal: Arc<Journal>,
    artifact: Artifact,
    destroyed: bool,
    output_bytes: Vec<u8>,
    output: Box<SeOutput>,
}
impl WorkerState {
    fn invoke(&mut self, kind: CallKind, input: &[u8]) -> Result<Vec<u8>, PluginError> {
        let token = self.journal.begin(
            Some(self.context.id.clone()),
            self.context.generation.clone(),
            self.artifact.sha256.clone(),
            kind.name().into(),
        )?;
        self.metrics
            .callbacks_in_flight
            .fetch_add(1, Ordering::Relaxed);
        let result = self.foreign(kind, input);
        self.metrics
            .callbacks_in_flight
            .fetch_sub(1, Ordering::Relaxed);
        // The worker still owns the instance. Adapter cleanup or supervised
        // channel-disconnect cleanup consumes the obligation, never this
        // already-completed callback's unobserved error path.
        self.journal.end(&token)?;
        result
    }
    fn foreign(&mut self, kind: CallKind, input: &[u8]) -> Result<Vec<u8>, PluginError> {
        if matches!(kind, CallKind::Create) {
            if !self.instance.is_null() {
                return Err(error(PLUGIN_REENTRANT, "instance already created"));
            }
            let result = status(unsafe {
                self.api.create.expect("validated create")(
                    &*self.host,
                    SeSlice::from_bytes(input),
                    &mut self.instance,
                )
            });
            if result.is_err() && !self.instance.is_null() {
                let _ = self.foreign(CallKind::Destroy, &[]);
            }
            result?;
            if self.instance.is_null() {
                return Err(error(
                    PLUGIN_CALLBACK_ERROR,
                    "create returned a null instance",
                ));
            }
            return Ok(Vec::new());
        }
        if matches!(kind, CallKind::Destroy) {
            if self.destroyed || self.instance.is_null() {
                return Ok(Vec::new());
            }
            self.destroyed = true;
            self.metrics
                .destroy_attempts
                .fetch_add(1, Ordering::Relaxed);
            let instance = std::mem::replace(&mut self.instance, std::ptr::null_mut());
            status(unsafe { self.api.destroy.expect("validated destroy")(instance) })?;
            return Ok(Vec::new());
        }
        if self.instance.is_null() || self.destroyed {
            return Err(error(PLUGIN_DISABLED, "instance is not live"));
        }
        if matches!(kind, CallKind::Config) {
            let callback = self
                .api
                .on_config_changed
                .ok_or_else(|| error(PLUGIN_NOT_SUPPORTED, "configuration callback is absent"))?;
            status(unsafe { callback(self.instance, SeSlice::from_bytes(input)) })?;
            return Ok(Vec::new());
        }
        let callback = match kind {
            CallKind::Render => self.api.render_view,
            CallKind::Action => self.api.on_action,
            CallKind::Command => self.api.on_command,
            CallKind::Service => self.api.on_service_call,
            _ => None,
        }
        .ok_or_else(|| error(PLUGIN_NOT_SUPPORTED, "native callback is absent"))?;
        self.output_bytes.resize(MAX_JSON_BYTES, 0);
        *self.output = SeOutput {
            ptr: self.output_bytes.as_mut_ptr(),
            capacity: self.output_bytes.len(),
            len: 0,
        };
        status(unsafe { callback(self.instance, SeSlice::from_bytes(input), &mut *self.output) })?;
        if self.output.ptr != self.output_bytes.as_mut_ptr()
            || self.output.capacity != self.output_bytes.len()
            || self.output.len > self.output_bytes.len()
        {
            return Err(error(PLUGIN_CALLBACK_ERROR, "invalid native output buffer"));
        }
        Ok(self.output_bytes[..self.output.len].to_vec())
    }
}
