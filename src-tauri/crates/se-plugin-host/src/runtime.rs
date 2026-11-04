use crate::{
    abi_worker::{AbiWorker, CallKind, Metrics},
    adapter::{AdapterInput, CAbiPlugin, Leases},
    broker::PluginBroker,
    config, loader,
    registry::{Journal, Quarantine, Registry, RegistryEntry, Store},
    validation::{encode, error, io},
};
use cordis_core::{Context, FiberHandle, Plugin, PreparedPlugin};
use se_plugin_protocol::*;
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::{broadcast, watch, Mutex};
use uuid::Uuid;

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct WebInvokeParams {
    #[serde(rename = "command", alias = "commandId")]
    command_id: String,
    #[serde(default)]
    args: Value,
}
/// Release web in-flight admission on normal return, timeout, or caller cancellation.
struct WebRequestGuard {
    services: Arc<crate::host_services::HostServices>,
    session: String,
    request: String,
}
impl Drop for WebRequestGuard {
    fn drop(&mut self) {
        self.services
            .web
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .complete(&self.session, &self.request);
    }
}
#[derive(Debug, Clone)]
pub struct HostOptions {
    pub callback_timeout: Duration,
    pub queue_capacity: usize,
    pub control_timeout: Duration,
}
impl Default for HostOptions {
    fn default() -> Self {
        Self {
            callback_timeout: Duration::from_millis(2000),
            queue_capacity: 32,
            control_timeout: Duration::from_secs(20),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestartReason {
    pub plugin_id: Option<String>,
    pub generation: String,
    pub code: String,
}
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostDiagnostics {
    pub active_instances: usize,
    pub callbacks_in_flight: usize,
    pub broker_registrations: usize,
    pub view_count: usize,
    pub command_count: usize,
    pub destroy_attempts: usize,
    pub loaded_libraries: usize,
    pub host_generation_leases: usize,
    pub host_deliveries_inflight: usize,
    pub host_storage_tickets: usize,
    pub host_event_resources: usize,
    pub host_subscriptions: usize,
    pub host_ui_records: usize,
}
struct Active {
    manifest: Manifest,
    generation: String,
    worker: Arc<AbiWorker>,
    leases: Arc<Leases>,
    fiber: FiberHandle,
    events: VecDeque<(String, Vec<u8>, ViewSnapshot)>,
}
struct State {
    registry: Registry,
    active: BTreeMap<String, Active>,
    errors: BTreeMap<String, PluginError>,
    seq: u64,
    closed: bool,
}
pub struct PluginHost {
    self_ref: std::sync::OnceLock<std::sync::Weak<Self>>,
    root: PathBuf,
    boot_id: String,
    options: HostOptions,
    store: Store,
    journal: Arc<Journal>,
    context: Context,
    broker: Arc<PluginBroker>,
    host_services: Arc<crate::host_services::HostServices>,
    state: Mutex<State>,
    events: broadcast::Sender<PluginEvent>,
    restart: watch::Sender<Option<RestartReason>>,
    metrics: Arc<Metrics>,
    instances: AtomicUsize,
    views: AtomicUsize,
    commands: AtomicUsize,
    ready: AtomicBool,
    safe_mode: bool,
}

impl PluginHost {
    pub async fn open(
        profile_root: PathBuf,
        options: HostOptions,
    ) -> Result<Arc<Self>, PluginError> {
        if options.queue_capacity == 0
            || options.callback_timeout.is_zero()
            || options.control_timeout.is_zero()
        {
            return Err(error(
                PLUGIN_INVALID_REQUEST,
                "host deadlines and queue capacity must be positive",
            ));
        }
        let root = profile_root.join("plugins");
        let store = Store::open(root.clone())?;
        let root = store.root.clone();
        let mut registry = store.read()?;
        let boot_id = Uuid::new_v4().to_string();
        let (journal, recovery) = Journal::open(root.clone(), boot_id.clone())?;
        for entry in &mut registry.plugins {
            if entry.desired_enabled
                && (recovery.quarantine_all || recovery.plugin_ids.contains(&entry.id))
            {
                entry.quarantine = Some(Quarantine {
                    code: PLUGIN_QUARANTINED.into(),
                    message: "previous Plugin Core exited during native execution".into(),
                    at: now(),
                });
            }
        }
        store.write(&registry)?;
        gc(&root, &registry, &boot_id);
        let context = Context::new();
        let broker = Arc::new(PluginBroker::new());
        let host_services = Arc::new(crate::host_services::HostServices::new(
            profile_root,
            boot_id.clone(),
        ));
        broker.attach_host(host_services.clone());
        let _publication = context.provide(broker.clone()).map_err(|_| {
            error(
                PLUGIN_CALLBACK_ERROR,
                "cannot publish fixed PluginBroker service",
            )
        })?;
        let (events, _) = broadcast::channel(256);
        let (restart, _) = watch::channel(None);
        let host = Arc::new(Self {
            self_ref: std::sync::OnceLock::new(),
            root,
            boot_id,
            options,
            store,
            journal,
            context,
            broker,
            host_services,
            state: Mutex::new(State {
                registry,
                active: BTreeMap::new(),
                errors: BTreeMap::new(),
                seq: 0,
                closed: false,
            }),
            events,
            restart,
            metrics: Arc::new(Metrics::default()),
            instances: AtomicUsize::new(0),
            views: AtomicUsize::new(0),
            commands: AtomicUsize::new(0),
            ready: AtomicBool::new(false),
            safe_mode: recovery.safe_mode,
        });
        let _ = host.self_ref.set(Arc::downgrade(&host));
        {
            let mut state = host.state.lock().await;
            if !host.safe_mode {
                let _ = host.reconcile(&mut state).await;
            }
        }
        host.ready.store(true, Ordering::Release);
        // Timeout withdrawal is independent of the caller continuing to poll its RPC.
        let weak_delivery = Arc::downgrade(&host);
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_millis(10));
            loop {
                tick.tick().await;
                let Some(host) = weak_delivery.upgrade() else {
                    break;
                };
                if host.host_services.processes.take_changed() {
                    let mut state = host.state.lock().await;
                    let ids: Vec<_> = state.active.keys().cloned().collect();
                    for id in ids {
                        host.emit(
                            &mut state,
                            PluginEventType::HostUiChanged,
                            Some(&id),
                            None,
                            None,
                        );
                    }
                }
                if host.host_services.take_ui_changed() {
                    let mut state = host.state.lock().await;
                    host.emit(&mut state, PluginEventType::HostUiChanged, None, None, None);
                }
                // Clone leases only: never retain control state across native delivery.
                let workers: Vec<_> = {
                    let state = host.state.lock().await;
                    state
                        .active
                        .iter()
                        .map(|(id, active)| {
                            (
                                id.clone(),
                                active.generation.clone(),
                                active.worker.clone(),
                                active.manifest.clone(),
                            )
                        })
                        .collect()
                };
                for (id, generation, worker, manifest) in workers {
                    let Some(delivery) = host.host_services.next_delivery(&id, &generation) else {
                        continue;
                    };
                    let host = host.clone();
                    tokio::spawn(async move {
                        let encoded =
                            serde_json::to_vec(&delivery).expect("bounded host delivery JSON");
                        let result = worker.call(CallKind::HostDelivery, encoded).await;
                        let busy = result.as_ref().is_err_and(|e| e.code == PLUGIN_BUSY);
                        host.host_services
                            .delivery_done(&id, &generation, delivery, busy);
                        let result = result.and_then(|bytes| {
                            crate::validation::host_delivery_reply(&bytes, &manifest)
                        });
                        let mut state = host.state.lock().await;
                        if state
                            .active
                            .get(&id)
                            .is_none_or(|a| a.generation != generation)
                        {
                            return;
                        }
                        match result {
                            Ok(reply) => {
                                for view in reply.result.invalidate_views {
                                    if let Err(e) = host.refresh_view(&mut state, &id, &view).await
                                    {
                                        state.errors.insert(id.clone(), e);
                                        break;
                                    }
                                }
                            }
                            Err(_) if busy => {}
                            Err(e) => {
                                state.errors.insert(id.clone(), e.clone());
                                if crate::validation::fatal(&e) {
                                    host.quarantine(&mut state, &id, e).await;
                                }
                            }
                        }
                    });
                }
            }
        });
        let weak_faults = Arc::downgrade(&host);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(10));
            loop {
                interval.tick().await;
                let Some(host) = weak_faults.upgrade() else {
                    break;
                };
                let panics = std::mem::take(
                    &mut *host
                        .metrics
                        .panics
                        .lock()
                        .unwrap_or_else(|p| p.into_inner()),
                );
                if panics.is_empty() {
                    continue;
                }
                let mut state = host.state.lock().await;
                for reason in panics {
                    if let Some(id) = reason.plugin_id {
                        if state
                            .active
                            .get(&id)
                            .is_some_and(|active| active.generation == reason.generation)
                        {
                            host.quarantine(
                                &mut state,
                                &id,
                                error(PLUGIN_PANIC, "native SDK caught a panic"),
                            )
                            .await;
                        }
                    }
                }
            }
        });
        let weak = Arc::downgrade(&host);
        let mut receiver = host.restart_requested();
        tokio::spawn(async move {
            while receiver.changed().await.is_ok() {
                let reason = receiver.borrow_and_update().clone();
                let Some(host) = weak.upgrade() else {
                    break;
                };
                if let Some(reason) = reason {
                    let mut state = host.state.lock().await;
                    if let Some(id) = reason.plugin_id {
                        host.quarantine(
                            &mut state,
                            &id,
                            error(
                                &reason.code,
                                "native execution timed out; Plugin Core restart required",
                            ),
                        )
                        .await;
                    } else {
                        let ids: Vec<_> = state.active.keys().cloned().collect();
                        for id in ids {
                            host.quarantine(
                                &mut state,
                                &id,
                                error(
                                    PLUGIN_TIMEOUT,
                                    "unattributed loader timeout; Plugin Core restart required",
                                ),
                            )
                            .await;
                        }
                    }
                }
            }
        });
        Ok(host)
    }

    pub async fn list(&self) -> Result<PluginSnapshot, PluginError> {
        let state = self.control_lock().await?;
        self.ensure_readable(&state)?;
        Ok(PluginSnapshot {
            schema_version: SCHEMA_VERSION,
            boot_id: self.boot_id.clone(),
            seq: state.seq,
            plugins: state
                .registry
                .plugins
                .iter()
                .map(|entry| self.info(&state, entry))
                .collect(),
        })
    }
    pub async fn install(&self, request: InstallRequest) -> Result<PluginInfo, PluginError> {
        let host = self
            .self_ref
            .get()
            .and_then(std::sync::Weak::upgrade)
            .ok_or_else(|| error(PLUGIN_DISABLED, "plugin host is closed"))?;
        let deadline = std::time::Instant::now() + self.options.control_timeout;
        tokio::spawn(async move {
            crate::validation::CONTROL_DEADLINE
                .scope(deadline, host.install_inner(request))
                .await
        })
        .await
        .map_err(io)?
    }

    async fn install_inner(&self, request: InstallRequest) -> Result<PluginInfo, PluginError> {
        validate_install_request(&request)?;
        let mut state = self.control_lock().await?;
        self.ensure_open(&state)?;
        let (artifact, worker, manifest) = self.candidate(Path::new(&request.path), "").await?;
        drop(worker);
        if state
            .registry
            .plugins
            .iter()
            .any(|entry| entry.id == manifest.id)
        {
            return Err(error(
                PLUGIN_ALREADY_INSTALLED,
                "plugin ID is already installed",
            ));
        }
        let artifact_path = loader::package(&self.root, &artifact, &manifest)?;
        let mut registry = state.registry.clone();
        let config_revision = registry.allocate_config_revision()?;
        let entry = RegistryEntry {
            package: artifact
                .archive
                .as_ref()
                .map(|archive| archive.metadata(&self.root)),
            id: manifest.id.clone(),
            manifest,
            source_path: artifact.source,
            artifact_path,
            sha256: artifact.sha256,
            desired_enabled: false,
            config_revision,
            config_overrides: ConfigValues::new(),
            quarantine: None,
        };
        registry.plugins.push(entry.clone());
        self.store.write(&registry)?;
        state.registry = registry;
        self.emit(
            &mut state,
            PluginEventType::RegistryChanged,
            Some(&entry.id),
            None,
            None,
        );
        Ok(self.info(&state, &entry))
    }
    pub async fn set_enabled(&self, request: SetEnabledRequest) -> Result<PluginInfo, PluginError> {
        let host = self
            .self_ref
            .get()
            .and_then(std::sync::Weak::upgrade)
            .ok_or_else(|| error(PLUGIN_DISABLED, "plugin host is closed"))?;
        let deadline = std::time::Instant::now() + self.options.control_timeout;
        tokio::spawn(async move {
            crate::validation::CONTROL_DEADLINE
                .scope(deadline, host.set_enabled_inner(request))
                .await
        })
        .await
        .map_err(io)?
    }

    async fn set_enabled_inner(
        &self,
        request: SetEnabledRequest,
    ) -> Result<PluginInfo, PluginError> {
        validate_set_enabled_request(&request)?;
        let mut state = self.control_lock().await?;
        self.ensure_open(&state)?;
        let index = index(&state, &request.id)?;
        let old = state.registry.clone();
        state.registry.plugins[index].desired_enabled = request.enabled;
        if request.enabled {
            state.registry.plugins[index].quarantine = None;
            state.errors.remove(&request.id);
        }
        if let Err(e) = self.order(&state.registry) {
            state.registry = old;
            return Err(e);
        }
        if let Err(e) = self.store.write(&state.registry) {
            state.registry = old;
            return Err(e);
        }
        let result = self.reconcile(&mut state).await;
        self.emit(
            &mut state,
            PluginEventType::StateChanged,
            Some(&request.id),
            None,
            None,
        );
        result?;
        Ok(self.info(&state, &state.registry.plugins[index]))
    }
    pub async fn reload(&self, request: ReloadRequest) -> Result<PluginInfo, PluginError> {
        let host = self
            .self_ref
            .get()
            .and_then(std::sync::Weak::upgrade)
            .ok_or_else(|| error(PLUGIN_DISABLED, "plugin host is closed"))?;
        let deadline = std::time::Instant::now() + self.options.control_timeout;
        tokio::spawn(async move {
            crate::validation::CONTROL_DEADLINE
                .scope(deadline, host.reload_inner(request))
                .await
        })
        .await
        .map_err(io)?
    }

    async fn reload_inner(&self, request: ReloadRequest) -> Result<PluginInfo, PluginError> {
        validate_reload_request(&request)?;
        let mut state = self.control_lock().await?;
        self.ensure_open(&state)?;
        let index = index(&state, &request.id)?;
        let old_registry = state.registry.clone();
        let old = old_registry.plugins[index].clone();
        let source = request
            .path
            .as_ref()
            .map(PathBuf::from)
            .unwrap_or_else(|| old.source_path.clone());
        let (artifact, worker, manifest) = self.candidate(&source, &old.id).await?;
        drop(worker);
        if manifest.id != old.id {
            return Err(error(
                PLUGIN_INVALID_MANIFEST,
                "reload cannot change plugin ID",
            ));
        }
        if semver::Version::parse(&manifest.version).map_err(io)?
            < semver::Version::parse(&old.manifest.version).map_err(io)?
        {
            return Err(error(
                PLUGIN_INVALID_MANIFEST,
                "plugin downgrade is not allowed",
            ));
        }
        config::migration(&old, &manifest)?;
        let mut registry = state.registry.clone();
        let mut candidate = old.clone();
        if manifest.contributes.configuration != old.manifest.contributes.configuration {
            candidate.config_revision = registry.allocate_config_revision()?;
        }
        candidate.artifact_path = loader::package(&self.root, &artifact, &manifest)?;
        candidate.package = artifact
            .archive
            .as_ref()
            .map(|archive| archive.metadata(&self.root));
        candidate.source_path = artifact.source;
        candidate.sha256 = artifact.sha256;
        candidate.manifest = manifest;
        candidate.quarantine = None;
        registry.plugins[index] = candidate;
        state.registry = registry;
        self.order(&state.registry).inspect_err(|_| {
            state.registry = old_registry.clone();
        })?;
        self.stop_dependents(&mut state, &old.id).await;
        let result = self
            .reconcile(&mut state)
            .await
            .and_then(|()| self.store.write(&state.registry));
        if let Err(e) = result {
            self.stop_dependents(&mut state, &old.id).await;
            state.registry = old_registry;
            if crate::validation::fatal(&e) {
                self.quarantine(&mut state, &old.id, e.clone()).await;
            } else {
                if let Err(rebuild_error) = crate::validation::CONTROL_DEADLINE
                    .scope(
                        std::time::Instant::now() + self.options.control_timeout,
                        self.reconcile(&mut state),
                    )
                    .await
                {
                    self.quarantine(&mut state, &old.id, rebuild_error).await;
                }
            }
            return Err(PluginError::new(
                e.code,
                "reload failed; previous artifact/config restored (private state may have reset)",
            ));
        }
        self.emit(
            &mut state,
            PluginEventType::RegistryChanged,
            Some(&old.id),
            None,
            None,
        );
        Ok(self.info(&state, &state.registry.plugins[index]))
    }
    pub async fn uninstall(
        &self,
        request: UninstallRequest,
    ) -> Result<UninstallResult, PluginError> {
        let host = self
            .self_ref
            .get()
            .and_then(std::sync::Weak::upgrade)
            .ok_or_else(|| error(PLUGIN_DISABLED, "plugin host is closed"))?;
        let deadline = std::time::Instant::now() + self.options.control_timeout;
        tokio::spawn(async move {
            crate::validation::CONTROL_DEADLINE
                .scope(deadline, host.uninstall_inner(request))
                .await
        })
        .await
        .map_err(io)?
    }

    async fn uninstall_inner(
        &self,
        request: UninstallRequest,
    ) -> Result<UninstallResult, PluginError> {
        validate_uninstall_request(&request)?;
        let mut state = self.control_lock().await?;
        self.ensure_open(&state)?;
        let index = index(&state, &request.id)?;
        self.stop_dependents(&mut state, &request.id).await;
        let mut registry = state.registry.clone();
        registry.retain_config_revisions();
        registry.plugins.remove(index);
        if let Err(e) = self.store.write(&registry) {
            if let Err(rebuild) = crate::validation::CONTROL_DEADLINE
                .scope(
                    std::time::Instant::now() + self.options.control_timeout,
                    self.reconcile(&mut state),
                )
                .await
            {
                self.quarantine(&mut state, &request.id, rebuild).await;
            }
            return Err(e);
        }
        state.registry = registry;
        state.errors.remove(&request.id);
        self.reconcile(&mut state).await?;
        self.emit(
            &mut state,
            PluginEventType::RegistryChanged,
            Some(&request.id),
            None,
            None,
        );
        Ok(UninstallResult {
            id: request.id,
            removed: true,
        })
    }
    /// Install either a verified .seplugin archive or a legacy native library.
    pub async fn install_from_path(
        &self,
        path: impl AsRef<Path>,
    ) -> Result<PluginInfo, PluginError> {
        self.install(InstallRequest {
            path: path.as_ref().to_string_lossy().into_owned(),
        })
        .await
    }
    pub async fn web_open(
        &self,
        owner: &str,
        request: WebOpenRequest,
    ) -> Result<WebOpenReply, PluginError> {
        validate_web_open_request(&request)?;
        let state = self.control_lock().await?;
        self.ensure_open(&state)?;
        let active = live(&state, &request.id)?;
        if active.generation != request.generation {
            return Err(error(PLUGIN_STALE_GENERATION, "stale web open generation"));
        }
        let entry = &state.registry.plugins[index(&state, &request.id)?];
        let native = entry.artifact_path.clone();
        let manifest = entry.manifest.clone();
        let generation = active.generation.clone();
        let digest = entry
            .package
            .as_ref()
            .map(|p| p.digest.clone())
            .unwrap_or_else(|| entry.sha256.clone());
        drop(state);
        let package = tokio::task::spawn_blocking(move || {
            let root = crate::archive::package_root(&native)
                .ok_or_else(|| error(PLUGIN_INVALID_PACKAGE, "web view requires archive"))?;
            let descriptor =
                parse_package_descriptor(&std::fs::read(root.join("package.json")).map_err(io)?)?;
            Ok::<_, PluginError>((root, descriptor))
        })
        .await
        .map_err(io)??;
        if !self.host_services.is_live(&manifest.id, &generation) {
            return Err(error(PLUGIN_STALE_GENERATION, "web generation revoked"));
        }
        self.host_services
            .with_web(&manifest.id, &generation, |web| {
                web.open(
                    &self.boot_id,
                    owner,
                    &manifest,
                    &generation,
                    package,
                    &digest,
                    &request.view_id,
                )
            })
    }
    fn web_session(
        &self,
        owner: &str,
        session_id: &str,
    ) -> Result<crate::web_sessions::Session, PluginError> {
        let session = self
            .host_services
            .web
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(session_id)?;
        if session.owner != owner
            || !self
                .host_services
                .is_live(&session.reply.plugin_id, &session.reply.generation)
        {
            return Err(error(PLUGIN_UI_SESSION_EXPIRED, "web session is not live"));
        }
        Ok(session)
    }
    pub async fn web_close(
        &self,
        owner: &str,
        request: WebCloseRequest,
    ) -> Result<(), PluginError> {
        validate_web_close_request(&request)?;
        self.web_session(owner, &request.session_id)?;
        self.host_services
            .web
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .close(&request.session_id);
        Ok(())
    }
    pub async fn web_asset(
        &self,
        owner: &str,
        request: WebAssetRequest,
    ) -> Result<WebAssetReply, PluginError> {
        let session = self.web_session(owner, &request.session_id)?;
        let session_id = request.session_id.clone();
        let reply = tokio::task::spawn_blocking(move || {
            crate::web_sessions::WebSessions::asset(&session, &request)
        })
        .await
        .map_err(io)??;
        self.web_session(owner, &session_id)?;
        Ok(reply)
    }
    pub async fn web_read(
        &self,
        owner: &str,
        request: WebReadRequest,
    ) -> Result<WebReadReply, PluginError> {
        self.web_session(owner, &request.session_id)?;
        self.host_services
            .web
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .read(&request)
    }
    pub async fn web_request(
        &self,
        owner: &str,
        request: WebRequest,
    ) -> Result<WebRequestReply, PluginError> {
        self.web_session(owner, &request.session_id)?;
        let session = self
            .host_services
            .web
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .admit(&request)?;
        let admission = WebRequestGuard {
            services: self.host_services.clone(),
            session: request.session_id.clone(),
            request: request.request_id.clone(),
        };
        let result = tokio::time::timeout(
            Duration::from_millis(WEB_REQUEST_TIMEOUT_MS),
            self.web_request_inner(&session, &request),
        )
        .await
        .unwrap_or_else(|_| Err(error(PLUGIN_TIMEOUT, "web request timed out")));
        drop(admission);
        self.web_session(owner, &request.session_id)?;
        let value = result?;
        if serde_json::to_vec(&value).map_err(io)?.len() > MAX_WEB_PAYLOAD_BYTES {
            return Err(error(PLUGIN_INVALID_REQUEST, "web reply too large"));
        }
        Ok(value)
    }
    async fn web_request_inner(
        &self,
        session: &crate::web_sessions::Session,
        request: &WebRequest,
    ) -> Result<Value, PluginError> {
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct WebViewDispatchParams {
            action: String,
            #[serde(default)]
            payload: Value,
        }
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct WebConfigSetParams {
            expected_revision: u64,
            values: std::collections::BTreeMap<String, ConfigValue>,
        }
        let plugin = &session.reply.plugin_id;
        let generation = &session.reply.generation;
        let parse_error = || error(PLUGIN_INVALID_REQUEST, "invalid web method params");
        match request.method.as_str() {
            "invoke" => {
                let args: WebInvokeParams =
                    serde_json::from_value(request.params.clone()).map_err(|_| parse_error())?;
                Ok(self
                    .execute_command(ExecuteCommandRequest {
                        id: plugin.clone(),
                        generation: generation.clone(),
                        command_id: args.command_id,
                        args: args.args,
                    })
                    .await?
                    .result)
            }
            "view.dispatch" => {
                let args: WebViewDispatchParams =
                    serde_json::from_value(request.params.clone()).map_err(|_| parse_error())?;
                let state = self.control_lock().await?;
                let active = live(&state, plugin)?;
                if &active.generation != generation {
                    return Err(error(PLUGIN_STALE_GENERATION, "stale web generation"));
                }
                let input = json!({"schemaVersion":1,"service":"se.host.ui","version":1,"method":"event","input":{"viewId":session.reply.view_id,"sessionId":session.reply.session_id,"action":args.action,"payload":args.payload}});
                let bytes = active
                    .worker
                    .call(CallKind::Service, encode(&input)?)
                    .await?;
                let reply = parse_service_reply(&bytes)?;
                Ok(reply.result)
            }
            "config.get" => {
                let state = self.control_lock().await?;
                let active = live(&state, plugin)?;
                if &active.generation != generation {
                    return Err(error(PLUGIN_STALE_GENERATION, "stale web generation"));
                }
                serde_json::to_value(config::snapshot(
                    &state.registry.plugins[index(&state, plugin)?],
                )?)
                .map_err(io)
            }
            "config.set" => {
                let args: WebConfigSetParams =
                    serde_json::from_value(request.params.clone()).map_err(|_| parse_error())?;
                serde_json::to_value(
                    self.set_config_fenced(
                        SetConfigRequest {
                            id: plugin.clone(),
                            expected_revision: args.expected_revision,
                            values: args.values,
                        },
                        Some(generation),
                    )
                    .await?,
                )
                .map_err(io)
            }
            "context.snapshot" => {
                let args: HostContextSnapshotRequest =
                    serde_json::from_value(request.params.clone()).map_err(|_| parse_error())?;
                self.host_services
                    .web_context_snapshot(plugin, generation, &session.owner, args)
            }
            "theme.get" => self
                .host_services
                .web_theme(plugin, generation, &session.owner),
            method if method.starts_with("storage.") => {
                let method = method.trim_start_matches("storage.");
                self.host_services
                    .web_storage(
                        plugin,
                        generation,
                        &session.reply.session_id,
                        &request.request_id,
                        method,
                        &request.params,
                    )
                    .await
            }
            method if method.starts_with("feedback.") => self.host_services.web_feedback(
                plugin,
                generation,
                &session.owner,
                method.trim_start_matches("feedback."),
                &request.params,
            ),
            _ => Err(error(PLUGIN_NOT_SUPPORTED, "unknown web bridge method")),
        }
    }
    pub async fn web_publish_theme(
        &self,
        owner: &str,
        theme: ThemeSnapshot,
    ) -> Result<(), PluginError> {
        self.host_services
            .web
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .publish_theme(owner, theme)
    }
    pub async fn process_list(
        &self,
        plugin_id: &str,
    ) -> Result<std::collections::BTreeMap<String, HostProcessStatus>, PluginError> {
        let state = self.control_lock().await?;
        let generation = &live(&state, plugin_id)?.generation;
        self.host_services.processes.list(plugin_id, generation)
    }
    pub async fn process_logs(
        &self,
        plugin_id: &str,
        id: &str,
        tail: u32,
    ) -> Result<HostProcessLogsReply, PluginError> {
        let state = self.control_lock().await?;
        let generation = &live(&state, plugin_id)?.generation;
        self.host_services
            .processes
            .logs(plugin_id, generation, id, tail)
    }
    pub async fn process_control(
        &self,
        plugin_id: &str,
        id: &str,
        action: &str,
    ) -> Result<HostProcessStatus, PluginError> {
        let state = self.control_lock().await?;
        let generation = &live(&state, plugin_id)?.generation;
        self.host_services
            .processes
            .control(plugin_id, generation, id, action)
    }

    pub async fn get_view(&self, request: GetViewRequest) -> Result<ViewSnapshot, PluginError> {
        validate_get_view_request(&request)?;
        let state = self.control_lock().await?;
        self.ensure_open(&state)?;
        let active = live(&state, &request.id)?;
        let result = active
            .leases
            .snapshots
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(&request.view_id)
            .cloned()
            .ok_or_else(|| error(PLUGIN_NOT_FOUND, "view is not declared"));
        result
    }
    pub async fn dispatch_action(&self, request: ActionEvent) -> Result<ViewSnapshot, PluginError> {
        let host = self
            .self_ref
            .get()
            .and_then(std::sync::Weak::upgrade)
            .ok_or_else(|| error(PLUGIN_DISABLED, "plugin host is closed"))?;
        let deadline = std::time::Instant::now() + self.options.control_timeout;
        tokio::spawn(async move {
            crate::validation::CONTROL_DEADLINE
                .scope(deadline, host.dispatch_action_inner(request))
                .await
        })
        .await
        .map_err(io)?
    }

    async fn dispatch_action_inner(
        &self,
        request: ActionEvent,
    ) -> Result<ViewSnapshot, PluginError> {
        validate_action_event(&request)?;
        let mut state = self.control_lock().await?;
        self.ensure_open(&state)?;
        let payload = encode(&request)?;
        let active = live(&state, &request.plugin_id)?;
        if active.generation != request.generation {
            return Err(error(
                PLUGIN_STALE_GENERATION,
                "action belongs to an old generation",
            ));
        }
        if let Some((_, previous, snapshot)) = active
            .events
            .iter()
            .find(|(id, _, _)| *id == request.event_id)
        {
            return if *previous == payload {
                Ok(snapshot.clone())
            } else {
                Err(error(
                    PLUGIN_INVALID_ACTION,
                    "eventId was reused with a different payload",
                ))
            };
        }
        let snapshot = active
            .leases
            .snapshots
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(&request.view_id)
            .cloned()
            .ok_or_else(|| error(PLUGIN_NOT_FOUND, "unknown view"))?;
        validate_action_against_snapshot(&request, &snapshot)?;
        let result = active
            .worker
            .call(CallKind::Action, payload.clone())
            .await
            .and_then(|bytes| crate::validation::tree(&bytes, &request.view_id));
        let tree = match result {
            Ok(tree) => tree,
            Err(e) => {
                if crate::validation::fatal(&e) {
                    self.quarantine(&mut state, &request.plugin_id, e.clone())
                        .await;
                }
                return Err(e);
            }
        };
        let snapshot = ViewSnapshot {
            revision: next_revision(snapshot.revision)?,
            tree,
            ..snapshot
        };
        let active = state
            .active
            .get_mut(&request.plugin_id)
            .expect("live under control lock");
        active
            .leases
            .snapshots
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(request.view_id.clone(), snapshot.clone());
        active
            .events
            .push_back((request.event_id, payload, snapshot.clone()));
        if active.events.len() > 32 {
            active.events.pop_front();
        }
        self.emit(
            &mut state,
            PluginEventType::ViewChanged,
            Some(&request.plugin_id),
            Some(&request.view_id),
            None,
        );
        Ok(snapshot)
    }
    pub async fn execute_command(
        &self,
        request: ExecuteCommandRequest,
    ) -> Result<CommandReply, PluginError> {
        let host = self
            .self_ref
            .get()
            .and_then(std::sync::Weak::upgrade)
            .ok_or_else(|| error(PLUGIN_DISABLED, "plugin host is closed"))?;
        let deadline = std::time::Instant::now() + self.options.control_timeout;
        tokio::spawn(async move {
            crate::validation::CONTROL_DEADLINE
                .scope(deadline, host.execute_command_inner(request))
                .await
        })
        .await
        .map_err(io)?
    }

    async fn execute_command_inner(
        &self,
        request: ExecuteCommandRequest,
    ) -> Result<CommandReply, PluginError> {
        validate_execute_command_request(&request)?;
        let mut state = self.control_lock().await?;
        self.ensure_open(&state)?;
        self.execute_command_locked(&mut state, request).await
    }

    async fn execute_command_locked(
        &self,
        state: &mut State,
        request: ExecuteCommandRequest,
    ) -> Result<CommandReply, PluginError> {
        let index = index(state, &request.id)?;
        if !state.registry.plugins[index]
            .manifest
            .contributes
            .commands
            .iter()
            .any(|command| command.id == request.command_id)
        {
            return Err(error(PLUGIN_NOT_FOUND, "command is not declared"));
        }
        let active = live(state, &request.id)?;
        if active.generation != request.generation {
            return Err(error(
                PLUGIN_STALE_GENERATION,
                "command belongs to an old generation",
            ));
        }
        let input = CommandRequest {
            schema_version: SCHEMA_VERSION,
            command_id: request.command_id,
            args: request.args,
        };
        let result = active
            .worker
            .call(CallKind::Command, encode(&input)?)
            .await
            .and_then(|bytes| parse_command_reply(&bytes));
        let reply = match result {
            Ok(reply) => reply,
            Err(e) => {
                if crate::validation::fatal(&e) {
                    self.quarantine(state, &request.id, e.clone()).await;
                }
                return Err(e);
            }
        };
        for view in &reply.invalidate_views {
            if !state.registry.plugins[index]
                .manifest
                .contributes
                .views
                .iter()
                .any(|declared| declared.id == *view)
            {
                return Err(error(
                    PLUGIN_CALLBACK_ERROR,
                    "command invalidates an undeclared view",
                ));
            }
        }
        for view in &reply.invalidate_views {
            self.refresh_view(state, &request.id, view).await?;
        }
        Ok(reply)
    }
    pub async fn get_config(
        &self,
        request: GetConfigRequest,
    ) -> Result<ConfigSnapshot, PluginError> {
        validate_get_config_request(&request)?;
        let state = self.control_lock().await?;
        self.ensure_readable(&state)?;
        config::snapshot(&state.registry.plugins[index(&state, &request.id)?])
    }
    pub async fn set_config(
        &self,
        request: SetConfigRequest,
    ) -> Result<ConfigSnapshot, PluginError> {
        let host = self
            .self_ref
            .get()
            .and_then(std::sync::Weak::upgrade)
            .ok_or_else(|| error(PLUGIN_DISABLED, "plugin host is closed"))?;
        let deadline = std::time::Instant::now() + self.options.control_timeout;
        tokio::spawn(async move {
            crate::validation::CONTROL_DEADLINE
                .scope(deadline, host.set_config_inner(request))
                .await
        })
        .await
        .map_err(io)?
    }

    async fn set_config_inner(
        &self,
        request: SetConfigRequest,
    ) -> Result<ConfigSnapshot, PluginError> {
        self.set_config_fenced(request, None).await
    }
    async fn set_config_fenced(
        &self,
        request: SetConfigRequest,
        generation: Option<&str>,
    ) -> Result<ConfigSnapshot, PluginError> {
        let mut state = self.control_lock().await?;
        self.ensure_open(&state)?;
        if let Some(generation) = generation {
            if live(&state, &request.id)?.generation != generation {
                return Err(error(
                    PLUGIN_STALE_GENERATION,
                    "stale web config generation",
                ));
            }
        }
        let index = index(&state, &request.id)?;
        let old_registry = state.registry.clone();
        let old = state.registry.plugins[index].clone();
        let Some(candidate) = config::update(&old, &request)? else {
            return config::snapshot(&old);
        };
        let old_values = config::snapshot(&old)?.values;
        let new_values = config::snapshot(&candidate)?.values;
        let changed = new_values
            .iter()
            .filter(|(key, value)| old_values.get(*key) != Some(*value))
            .map(|(key, _)| key.clone())
            .collect();
        let mut staged = BTreeMap::new();
        let result = if let Some(active) = state.active.get(&old.id) {
            let worker = active.worker.clone();
            let generation = active.generation.clone();
            let old_snapshots = active
                .leases
                .snapshots
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .clone();
            async {
                worker
                    .call(
                        CallKind::Config,
                        encode(&config::payload(&candidate, changed)?)?,
                    )
                    .await?;
                for view in &candidate.manifest.contributes.views {
                    if view.kind == Some(ViewKind::Web) {
                        continue;
                    }
                    let request = RenderRequest {
                        schema_version: SCHEMA_VERSION,
                        view_id: view.id.clone(),
                    };
                    let tree = crate::validation::tree(
                        &worker.call(CallKind::Render, encode(&request)?).await?,
                        &view.id,
                    )?;
                    let revision = next_revision(
                        old_snapshots
                            .get(&view.id)
                            .map_or(0, |snapshot| snapshot.revision),
                    )?;
                    staged.insert(
                        view.id.clone(),
                        ViewSnapshot {
                            plugin_id: old.id.clone(),
                            generation: generation.clone(),
                            revision,
                            config_revision: candidate.config_revision,
                            tree,
                        },
                    );
                }
                Ok::<(), PluginError>(())
            }
            .await
        } else {
            Ok(())
        };
        let result = result.and_then(|()| {
            let mut registry = old_registry.clone();
            registry.plugins[index] = candidate.clone();
            registry.retain_config_revisions();
            self.store.write(&registry)?;
            state.registry = registry;
            Ok(())
        });
        if let Err(e) = result {
            self.stop_dependents(&mut state, &old.id).await;
            state.registry = old_registry;
            if crate::validation::fatal(&e) {
                self.quarantine(&mut state, &old.id, e.clone()).await;
            } else {
                if let Err(rebuild_error) = crate::validation::CONTROL_DEADLINE
                    .scope(
                        std::time::Instant::now() + self.options.control_timeout,
                        self.reconcile(&mut state),
                    )
                    .await
                {
                    self.quarantine(&mut state, &old.id, rebuild_error).await;
                }
            }
            return Err(PluginError::new(e.code, "configuration update failed; old persisted configuration retained; private state may have reset during rebuild"));
        }
        if let Some(active) = state.active.get_mut(&old.id) {
            *active
                .leases
                .snapshots
                .lock()
                .unwrap_or_else(|p| p.into_inner()) = staged;
            active.events.clear();
        }
        self.emit(
            &mut state,
            PluginEventType::ConfigChanged,
            Some(&old.id),
            None,
            Some(candidate.config_revision),
        );
        config::snapshot(&candidate)
    }
    pub fn is_ready(&self) -> bool {
        self.ready.load(Ordering::Acquire) && self.restart.borrow().is_none()
    }
    pub fn safe_mode(&self) -> bool {
        self.safe_mode
    }
    pub async fn publish_host_context(
        &self,
        owner: &str,
        request: HostContextPublishRequest,
    ) -> Result<HostContextAck, PluginError> {
        self.host_services.publish(owner, request)
    }
    pub fn heartbeat_host_context(
        &self,
        owner: &str,
        source_id: &str,
    ) -> Result<HostContextAck, PluginError> {
        self.host_services.heartbeat(owner, source_id)
    }
    pub fn detach_host_context(&self, owner: &str) {
        self.host_services.detach(owner);
    }
    pub async fn host_ui_snapshot(&self) -> Result<HostUiSnapshot, PluginError> {
        self.host_services.ui_snapshot()
    }
    pub async fn dismiss_host_ui(
        &self,
        request: HostUiDismissRequest,
    ) -> Result<HostUiDismissResult, PluginError> {
        self.host_services.dismiss(request)
    }
    pub async fn execute_contribution(
        &self,
        request: ExecuteContributionRequest,
    ) -> Result<CommandReply, PluginError> {
        let host = self
            .self_ref
            .get()
            .and_then(std::sync::Weak::upgrade)
            .ok_or_else(|| error(PLUGIN_DISABLED, "plugin host is closed"))?;
        let deadline = std::time::Instant::now() + self.options.control_timeout;
        tokio::spawn(async move {
            crate::validation::CONTROL_DEADLINE
                .scope(deadline, async {
                    let mut state = host.control_lock().await?;
                    host.ensure_open(&state)?;
                    let (command_id, args) = host.host_services.resolve_action(&request)?;
                    host.execute_command_locked(
                        &mut state,
                        ExecuteCommandRequest {
                            id: request.plugin_id,
                            generation: request.generation,
                            command_id,
                            args,
                        },
                    )
                    .await
                })
                .await
        })
        .await
        .map_err(io)?
    }

    pub fn subscribe(&self) -> broadcast::Receiver<PluginEvent> {
        self.events.subscribe()
    }
    pub fn restart_requested(&self) -> watch::Receiver<Option<RestartReason>> {
        self.restart.subscribe()
    }
    pub fn active_resources(&self) -> usize {
        self.instances.load(Ordering::Acquire)
            + self.metrics.callbacks_in_flight.load(Ordering::Acquire)
    }
    pub fn diagnostics(&self) -> HostDiagnostics {
        HostDiagnostics {
            active_instances: self.instances.load(Ordering::Acquire),
            callbacks_in_flight: self.metrics.callbacks_in_flight.load(Ordering::Acquire),
            broker_registrations: self.broker.count(),
            view_count: self.views.load(Ordering::Acquire),
            command_count: self.commands.load(Ordering::Acquire),
            destroy_attempts: self.metrics.destroy_attempts.load(Ordering::Acquire),
            loaded_libraries: self.metrics.loaded_libraries.load(Ordering::Acquire),
            host_generation_leases: self.host_services.counts().0,
            host_deliveries_inflight: self.host_services.counts().1,
            host_storage_tickets: self.host_services.storage_count(),
            host_event_resources: self.host_services.context_counts().0,
            host_subscriptions: self.host_services.context_counts().1,
            host_ui_records: self.host_services.ui_count(),
        }
    }
    pub async fn shutdown(&self) -> Result<(), PluginError> {
        let host = self
            .self_ref
            .get()
            .and_then(std::sync::Weak::upgrade)
            .ok_or_else(|| error(PLUGIN_DISABLED, "plugin host is closed"))?;
        let deadline = std::time::Instant::now() + self.options.control_timeout;
        tokio::spawn(async move {
            crate::validation::CONTROL_DEADLINE
                .scope(deadline, host.shutdown_inner())
                .await
        })
        .await
        .map_err(io)?
    }
    async fn shutdown_inner(&self) -> Result<(), PluginError> {
        let mut state = self.control_lock().await?;
        if state.closed {
            return Ok(());
        }
        self.ready.store(false, Ordering::Release);
        let ids = state.active.keys().cloned().collect();
        self.stop_set(&mut state, &ids).await;
        state.closed = true;
        self.host_services.processes.shutdown().await;
        if self.metrics.callbacks_in_flight.load(Ordering::Acquire) == 0
            && self.restart.borrow().is_none()
        {
            self.journal.clean_exit()?;
        }
        Ok(())
    }

    async fn control_lock(&self) -> Result<tokio::sync::MutexGuard<'_, State>, PluginError> {
        let deadline = crate::validation::deadline(self.options.control_timeout);
        tokio::time::timeout_at(deadline.into(), self.state.lock())
            .await
            .map_err(|_| error(PLUGIN_BUSY, "control operation expired before admission"))
    }
    fn ensure_readable(&self, state: &State) -> Result<(), PluginError> {
        if state.closed {
            Err(error(PLUGIN_DISABLED, "plugin host is shut down"))
        } else {
            Ok(())
        }
    }
    fn ensure_open(&self, state: &State) -> Result<(), PluginError> {
        if state.closed {
            Err(error(PLUGIN_DISABLED, "plugin host is shut down"))
        } else if self.restart.borrow().is_some() {
            Err(error(PLUGIN_QUARANTINED, "Plugin Core restart is required"))
        } else {
            Ok(())
        }
    }
    fn order(&self, registry: &Registry) -> Result<Vec<String>, PluginError> {
        PluginBroker::order(
            &registry
                .plugins
                .iter()
                .filter(|entry| entry.desired_enabled && entry.quarantine.is_none())
                .map(|entry| entry.manifest.clone())
                .collect::<Vec<_>>(),
        )
    }
    fn info(&self, state: &State, entry: &RegistryEntry) -> PluginInfo {
        let active = state.active.get(&entry.id);
        let plugin_state = if entry.quarantine.is_some() {
            PluginState::Quarantined
        } else if !entry.desired_enabled {
            PluginState::Disabled
        } else if active.is_some() {
            PluginState::Active
        } else if entry
            .manifest
            .services
            .requires
            .iter()
            .any(|s| s.name == "se.host.secrets")
        {
            PluginState::Pending
        } else if state.errors.contains_key(&entry.id) {
            PluginState::Failed
        } else {
            PluginState::Pending
        };
        PluginInfo {
            id: entry.id.clone(),
            manifest: entry.manifest.clone(),
            desired_enabled: entry.desired_enabled,
            state: plugin_state,
            generation: active.map(|active| active.generation.clone()),
            config_revision: entry.config_revision,
            sha256: entry.sha256.clone(),
            last_error: entry
                .quarantine
                .as_ref()
                .map(|q| PluginError::new(q.code.clone(), q.message.clone()))
                .or_else(|| state.errors.get(&entry.id).cloned())
                .map(crate::registry::normalize_diagnostic),
        }
    }
    fn emit(
        &self,
        state: &mut State,
        kind: PluginEventType,
        id: Option<&str>,
        view: Option<&str>,
        revision: Option<u64>,
    ) {
        state.seq += 1;
        let event = PluginEvent {
            schema_version: SCHEMA_VERSION,
            boot_id: self.boot_id.clone(),
            seq: state.seq,
            kind,
            plugin_id: id.map(str::to_owned),
            generation: id
                .and_then(|id| state.active.get(id).map(|active| active.generation.clone())),
            view_id: view.map(str::to_owned),
            config_revision: revision,
        };
        let _ = self.events.send(event);
    }
    async fn candidate(
        &self,
        source: &Path,
        id: &str,
    ) -> Result<(loader::Artifact, Arc<AbiWorker>, Manifest), PluginError> {
        let artifact = loader::stage(&self.root, &self.boot_id, source)?;
        let (worker, manifest) = AbiWorker::start(
            artifact.clone(),
            id.into(),
            Uuid::new_v4().to_string(),
            self.broker.clone(),
            self.journal.clone(),
            self.options.clone(),
            self.metrics.clone(),
            self.restart.clone(),
        )
        .await?;
        if artifact
            .archive
            .as_ref()
            .is_some_and(|a| a.manifest != manifest)
        {
            drop(worker);
            return Err(error(
                PLUGIN_INVALID_PACKAGE,
                "ABI manifest differs from archive manifest",
            ));
        }
        Ok((artifact, worker, manifest))
    }
    async fn activate(&self, state: &mut State, entry: &RegistryEntry) -> Result<(), PluginError> {
        let artifact = if let Some(package) = &entry.package {
            crate::archive::stage_published(
                &self.root,
                &self.boot_id,
                &entry.artifact_path,
                package,
            )?
        } else {
            loader::stage(&self.root, &self.boot_id, &entry.artifact_path)?
        };
        if artifact.sha256 != entry.sha256 {
            return Err(error(
                PLUGIN_STORE_CORRUPT,
                "installed artifact hash mismatch",
            ));
        }
        let generation = Uuid::new_v4().to_string();
        let (worker, manifest) = AbiWorker::start(
            artifact,
            entry.id.clone(),
            generation.clone(),
            self.broker.clone(),
            self.journal.clone(),
            self.options.clone(),
            self.metrics.clone(),
            self.restart.clone(),
        )
        .await?;
        if manifest != entry.manifest {
            return Err(error(PLUGIN_STORE_CORRUPT, "installed manifest changed"));
        }
        worker.set_teardown_deadline(crate::validation::deadline(self.options.control_timeout));
        let leases = Arc::new(Leases::default());
        let input = AdapterInput {
            manifest: manifest.clone(),
            generation: generation.clone(),
            config: config::payload(entry, Vec::new())?,
            worker: worker.clone(),
            broker: self.broker.clone(),
            leases: leases.clone(),
            deadline: crate::validation::deadline(self.options.control_timeout),
        };
        let adapter = CAbiPlugin;
        let input = adapter.prepare(input)?;
        let fiber = match self
            .context
            .spawn(PreparedPlugin::from_input(adapter, input))
            .await
        {
            Ok(fiber) => fiber,
            Err(_) => {
                self.broker.withdraw(&entry.id, &generation);
                return Err(worker.last_error().unwrap_or_else(|| {
                    error(
                        PLUGIN_CALLBACK_ERROR,
                        "native plugin activation failed; all leases rolled back",
                    )
                }));
            }
        };
        self.host_services.processes.activate(
            &entry.id,
            &generation,
            crate::archive::package_root(&entry.artifact_path),
            &entry.manifest.contributes.processes,
        )?;
        state.active.insert(
            entry.id.clone(),
            Active {
                manifest: manifest.clone(),
                generation,
                worker,
                leases,
                fiber,
                events: VecDeque::new(),
            },
        );
        state.errors.remove(&entry.id);
        self.update_counts(state);
        self.emit(
            state,
            PluginEventType::StateChanged,
            Some(&entry.id),
            None,
            None,
        );
        Ok(())
    }
    async fn reconcile(&self, state: &mut State) -> Result<(), PluginError> {
        let order = self.order(&state.registry)?;
        let wanted: BTreeSet<_> = order.iter().cloned().collect();
        let stops: Vec<_> = state
            .active
            .keys()
            .filter(|id| !wanted.contains(*id))
            .cloned()
            .collect();
        self.stop_set(state, &stops.into_iter().collect()).await;
        let mut first_error = None;
        for id in order {
            if state.active.contains_key(&id) {
                continue;
            }
            let entry = state
                .registry
                .plugins
                .iter()
                .find(|entry| entry.id == id)
                .expect("graph entry")
                .clone();
            if !self.dependencies_live(state, &entry.manifest) {
                continue;
            }
            if let Err(e) = self.activate(state, &entry).await {
                state.errors.insert(id.clone(), e.clone());
                if crate::validation::fatal(&e) {
                    self.quarantine(state, &id, e.clone()).await;
                }
                if first_error.is_none() {
                    first_error = Some(e);
                }
            }
        }
        for entry in &state.registry.plugins {
            if entry.desired_enabled
                && !state.active.contains_key(&entry.id)
                && entry
                    .manifest
                    .services
                    .requires
                    .iter()
                    .any(|s| s.name == "se.host.secrets")
            {
                state.errors.insert(
                    entry.id.clone(),
                    error(
                        PLUGIN_NOT_SUPPORTED,
                        "se.host.secrets requires an unavailable OS credential backend",
                    ),
                );
            }
        }
        self.update_counts(state);
        first_error.map_or(Ok(()), Err)
    }
    fn dependencies_live(&self, state: &State, manifest: &Manifest) -> bool {
        manifest.services.requires.iter().all(|required| {
            se_plugin_protocol::known_host_service(&required.name, required.version)
                == Some(se_plugin_protocol::HostServiceAvailability::Available)
                || state.registry.plugins.iter().any(|provider| {
                    state.active.contains_key(&provider.id)
                        && provider.manifest.services.provides.iter().any(|provided| {
                            provided.name == required.name && provided.version == required.version
                        })
                })
        })
    }
    async fn stop_one(&self, state: &mut State, id: &str) {
        if let Some(active) = state.active.remove(id) {
            // Close admission immediately, before awaiting cordis teardown.
            self.broker.withdraw(id, &active.generation);
            active.leases.live.store(false, Ordering::Release);
            active
                .leases
                .snapshots
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .clear();
            self.update_counts(state);
            active
                .worker
                .set_teardown_deadline(crate::validation::deadline(self.options.control_timeout));
            let _ = active.fiber.dispose().await;
        }
    }
    async fn stop_dependents(&self, state: &mut State, id: &str) {
        let mut affected = BTreeSet::from([id.to_owned()]);
        loop {
            let before = affected.len();
            for (consumer_id, active) in &state.active {
                if active.manifest.services.requires.iter().any(|required| {
                    state.active.iter().any(|(provider_id, provider)| {
                        affected.contains(provider_id)
                            && provider.manifest.services.provides.iter().any(|provided| {
                                provided.name == required.name
                                    && provided.version == required.version
                            })
                    })
                }) {
                    affected.insert(consumer_id.clone());
                }
            }
            if affected.len() == before {
                break;
            }
        }
        self.stop_set(state, &affected).await;
    }
    async fn stop_set(&self, state: &mut State, affected: &BTreeSet<String>) {
        let manifests: Vec<_> = state
            .active
            .values()
            .map(|active| active.manifest.clone())
            .collect();
        let order = PluginBroker::order(&manifests)
            .unwrap_or_else(|_| state.active.keys().cloned().collect());
        // Close every affected registration before the first foreign teardown wait.
        for id in affected {
            if let Some(active) = state.active.get(id) {
                self.broker.withdraw(id, &active.generation);
                active.leases.live.store(false, Ordering::Release);
            }
        }
        for id in order.into_iter().rev().filter(|id| affected.contains(id)) {
            self.stop_one(state, &id).await;
        }
    }
    async fn quarantine(&self, state: &mut State, id: &str, failure: PluginError) {
        self.stop_dependents(state, id).await;
        if let Some(entry) = state
            .registry
            .plugins
            .iter_mut()
            .find(|entry| entry.id == id)
        {
            entry.quarantine = Some(Quarantine {
                code: failure.code.clone(),
                message: failure.message.clone(),
                at: now(),
            });
        }
        state.errors.insert(id.into(), failure);
        let _ = self.store.write(&state.registry);
        self.emit(state, PluginEventType::StateChanged, Some(id), None, None);
    }
    fn update_counts(&self, state: &State) {
        self.instances.store(state.active.len(), Ordering::Release);
        self.views.store(
            state
                .active
                .values()
                .map(|active| {
                    active
                        .leases
                        .snapshots
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .len()
                })
                .sum(),
            Ordering::Release,
        );
        self.commands.store(
            state
                .registry
                .plugins
                .iter()
                .filter(|entry| state.active.contains_key(&entry.id))
                .map(|entry| entry.manifest.contributes.commands.len())
                .sum(),
            Ordering::Release,
        );
    }
    async fn refresh_view(
        &self,
        state: &mut State,
        id: &str,
        view_id: &str,
    ) -> Result<(), PluginError> {
        let active = live(state, id)?;
        let old = active
            .leases
            .snapshots
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(view_id)
            .cloned()
            .ok_or_else(|| error(PLUGIN_NOT_FOUND, "view is not declared"))?;
        let request = RenderRequest {
            schema_version: SCHEMA_VERSION,
            view_id: view_id.into(),
        };
        let result = active
            .worker
            .call(CallKind::Render, encode(&request)?)
            .await
            .and_then(|bytes| crate::validation::tree(&bytes, &request.view_id));
        let tree = match result {
            Ok(tree) => tree,
            Err(e) => {
                if crate::validation::fatal(&e) {
                    self.quarantine(state, id, e.clone()).await;
                }
                return Err(e);
            }
        };
        let snapshot = ViewSnapshot {
            revision: next_revision(old.revision)?,
            tree,
            ..old
        };
        state.active[id]
            .leases
            .snapshots
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(view_id.into(), snapshot);
        self.emit(
            state,
            PluginEventType::ViewChanged,
            Some(id),
            Some(view_id),
            None,
        );
        Ok(())
    }
}
fn index(state: &State, id: &str) -> Result<usize, PluginError> {
    state
        .registry
        .plugins
        .iter()
        .position(|entry| entry.id == id)
        .ok_or_else(|| error(PLUGIN_NOT_FOUND, "plugin is not installed"))
}
fn live<'a>(state: &'a State, id: &str) -> Result<&'a Active, PluginError> {
    index(state, id)?;
    state
        .active
        .get(id)
        .ok_or_else(|| error(PLUGIN_DISABLED, "plugin is not active"))
}
fn next_revision(revision: u64) -> Result<u64, PluginError> {
    if revision >= MAX_SAFE_INTEGER {
        Err(error(PLUGIN_CALLBACK_ERROR, "revision exhausted"))
    } else {
        Ok(revision + 1)
    }
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
fn gc(root: &Path, registry: &Registry, boot: &str) {
    // Do not traverse symlinks at any level. Recheck canonical containment before
    // deletion as well, rather than trusting only the final entry's file type.
    fn real_directory(path: &Path) -> Option<PathBuf> {
        let metadata = std::fs::symlink_metadata(path).ok()?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return None;
        }
        std::fs::canonicalize(path).ok()
    }
    fn contained_directory(path: &Path, parent: &Path) -> Option<PathBuf> {
        let canonical = real_directory(path)?;
        (canonical != parent && canonical.starts_with(parent)).then_some(canonical)
    }
    let Some(canonical_root) = real_directory(root) else {
        return;
    };
    if let Some(loaded) = contained_directory(&canonical_root.join("loaded"), &canonical_root) {
        if let Ok(entries) = std::fs::read_dir(&loaded) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let Some(name) = name.to_str() else {
                    continue;
                };
                if name != boot && Uuid::parse_str(name).is_ok() {
                    if let Some(path) = contained_directory(&entry.path(), &loaded) {
                        let _ = std::fs::remove_dir_all(path);
                    }
                }
            }
        }
    }
    // Only recognized, unreferenced package directories belong to the host.
    let Some(packages) = contained_directory(&canonical_root.join("packages"), &canonical_root)
    else {
        return;
    };
    let Ok(ids) = std::fs::read_dir(&packages) else {
        return;
    };
    for id in ids.flatten() {
        if !id.file_name().to_str().is_some_and(is_plugin_id) {
            continue;
        }
        let Some(id_path) = contained_directory(&id.path(), &packages) else {
            continue;
        };
        let Ok(versions) = std::fs::read_dir(&id_path) else {
            continue;
        };
        for version in versions.flatten() {
            if version
                .file_name()
                .to_str()
                .is_none_or(|name| semver::Version::parse(name).is_err())
            {
                continue;
            }
            let Some(version_path) = contained_directory(&version.path(), &id_path) else {
                continue;
            };
            let Ok(hashes) = std::fs::read_dir(&version_path) else {
                continue;
            };
            for hash in hashes.flatten() {
                if !hash.file_name().to_str().is_some_and(|name| {
                    name.len() == 64
                        && name
                            .bytes()
                            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                }) {
                    continue;
                }
                let Some(path) = contained_directory(&hash.path(), &packages) else {
                    continue;
                };
                let relative = path
                    .strip_prefix(&canonical_root)
                    .expect("contained package");
                let original = root.join(relative);
                if !registry.plugins.iter().any(|entry| {
                    entry
                        .package
                        .as_ref()
                        .is_some_and(|p| p.root == original || p.root == path)
                        || entry.artifact_path.starts_with(&original)
                        || entry.artifact_path.starts_with(&path)
                        || std::fs::canonicalize(&entry.artifact_path)
                            .is_ok_and(|artifact| artifact.starts_with(&path))
                }) {
                    let _ = std::fs::remove_dir_all(path);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn host_delivery_waits_for_callback_return() {
        let barrier = delivery_barrier().get_or_init(|| DeliveryBarrier {
            entered: std::sync::Barrier::new(2),
            release: std::sync::Barrier::new(2),
            returned: std::sync::atomic::AtomicBool::new(false),
            delivered: tokio::sync::Notify::new(),
        });
        let root = tempfile::tempdir().unwrap();
        let host = PluginHost::open(root.path().into(), HostOptions::default())
            .await
            .unwrap();
        let mut manifest = manifest(
            "test.host-delivery",
            &[],
            &["se.host.context", "se.host.events"],
        );
        manifest
            .contributes
            .event_subscriptions
            .push(EventSubscriptionContribution {
                id: "project".into(),
                topic: HostEventTopic::ProjectActiveChanged,
                scope: HostScope::Window,
            });
        install_fixture(&host, root.path(), manifest).await;
        let info = enabled(&host, "test.host-delivery", true).await.unwrap();
        let generation = info.generation.unwrap();
        let command_host = host.clone();
        let command = tokio::spawn(async move {
            command_host
                .execute_command(ExecuteCommandRequest {
                    id: "test.host-delivery".into(),
                    generation,
                    command_id: "echo".into(),
                    args: serde_json::Value::Null,
                })
                .await
        });
        tokio::task::spawn_blocking(move || barrier.entered.wait())
            .await
            .unwrap();
        let request = parse_host_context_publish_request(br#"{"sourceId":"00000000-0000-4000-8000-000000000010","sourceSequence":1,"reset":true,"snapshot":{"activeProject":null,"projectIds":[],"activeTerminalId":null,"activeSessionId":null,"terminals":[],"agentSessions":[],"focus":{"textInput":false,"terminal":false},"truncated":false},"events":[]}"#).unwrap();
        tokio::time::timeout(
            Duration::from_secs(1),
            host.publish_host_context("owner", request),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(!barrier.returned.load(Ordering::Acquire));
        barrier.release.wait();
        command.await.unwrap().unwrap();
        tokio::time::timeout(Duration::from_secs(1), barrier.delivered.notified())
            .await
            .unwrap();
        assert!(barrier.returned.load(Ordering::Acquire));
        enabled(&host, "test.host-delivery", false).await.unwrap();
        assert_eq!(host.host_services.counts(), (0, 0));
        host.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn secrets_dependency_stays_pending_with_unsupported_diagnostic() {
        let root = tempfile::tempdir().unwrap();
        let host = PluginHost::open(root.path().into(), HostOptions::default())
            .await
            .unwrap();
        install_fixture(
            &host,
            root.path(),
            manifest("test.secrets", &[], &["se.host.secrets"]),
        )
        .await;
        let info = enabled(&host, "test.secrets", true).await.unwrap();
        assert_eq!(info.state, PluginState::Pending);
        assert_eq!(info.last_error.unwrap().code, PLUGIN_NOT_SUPPORTED);
        assert!(info.generation.is_none());
        assert_eq!(host.diagnostics().host_generation_leases, 0);
        assert!(!root.path().join("plugins/data/test.secrets.json").exists());
        host.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn empty_host_lifecycle_has_no_resources() {
        let root = tempfile::tempdir().unwrap();
        let host = PluginHost::open(root.path().into(), HostOptions::default())
            .await
            .unwrap();
        assert!(host.list().await.unwrap().plugins.is_empty());
        assert_eq!(host.diagnostics(), HostDiagnostics::default());
        host.shutdown().await.unwrap();
        assert_eq!(host.active_resources(), 0);
    }
    fn destroyed() -> &'static std::sync::Mutex<Vec<String>> {
        static DESTROYED: std::sync::OnceLock<std::sync::Mutex<Vec<String>>> =
            std::sync::OnceLock::new();
        DESTROYED.get_or_init(Default::default)
    }
    struct DeliveryBarrier {
        entered: std::sync::Barrier,
        release: std::sync::Barrier,
        returned: std::sync::atomic::AtomicBool,
        delivered: tokio::sync::Notify,
    }
    fn delivery_barrier() -> &'static std::sync::OnceLock<DeliveryBarrier> {
        static BARRIER: std::sync::OnceLock<DeliveryBarrier> = std::sync::OnceLock::new();
        &BARRIER
    }
    struct Fixture {
        mode: String,
        config: u64,
    }
    impl se_plugin_abi::PluginV1 for Fixture {
        fn manifest() -> &'static [u8] {
            b"{}"
        }
        fn create(
            host: se_plugin_abi::Host,
            activation: &[u8],
        ) -> Result<Self, se_plugin_abi::SeStatus> {
            let value: serde_json::Value =
                serde_json::from_slice(activation).map_err(|_| se_plugin_abi::SEP_INVALID_JSON)?;
            if value["pluginId"]
                .as_str()
                .unwrap()
                .contains("broker-consumer")
            {
                let request=serde_json::to_vec(&serde_json::json!({"schemaVersion":1,"service":"broker.service","version":1,"method":"hello","input":{}})).unwrap();
                let mut output = vec![0; 4096];
                host.broker_call(&request, &mut output)?;
            }
            Ok(Self {
                mode: value["pluginId"].as_str().unwrap().to_owned(),
                config: value["config"]["revision"].as_u64().unwrap(),
            })
        }
        fn render_view(
            &mut self,
            request: &[u8],
            out: &mut se_plugin_abi::OutputWriter<'_>,
        ) -> Result<(), se_plugin_abi::SeStatus> {
            if self.mode.contains("invalid") || (self.mode.contains("rebuild") && self.config > 1) {
                return out.write(b"{}");
            }
            if self.mode.contains("panic") {
                panic!("fixture panic");
            }
            if self.mode.contains("timeout") {
                std::thread::sleep(Duration::from_millis(700));
            }
            let request: serde_json::Value = serde_json::from_slice(request).unwrap();
            out.write(&serde_json::to_vec(&serde_json::json!({"schemaVersion":1,"viewId":request["viewId"],"root":{"type":"text","id":"label","props":{"text":"hello"}}})).unwrap())
        }
        fn on_command(
            &mut self,
            _input: &[u8],
            out: &mut se_plugin_abi::OutputWriter<'_>,
        ) -> Result<(), se_plugin_abi::SeStatus> {
            if self.mode == "test.host-delivery" {
                let barrier = delivery_barrier().get().unwrap();
                barrier.entered.wait();
                barrier.release.wait();
                barrier.returned.store(true, Ordering::Release);
            }
            out.write(br#"{"schemaVersion":1,"result":null,"invalidateViews":[]}"#)
        }
        fn on_service_call(
            &mut self,
            _input: &[u8],
            out: &mut se_plugin_abi::OutputWriter<'_>,
        ) -> Result<(), se_plugin_abi::SeStatus> {
            if self.mode == "test.host-delivery" {
                let barrier = delivery_barrier().get().unwrap();
                assert!(barrier.returned.load(Ordering::Acquire));
                barrier.delivered.notify_one();
                return out.write(br#"{"schemaVersion":1,"result":{"invalidateViews":[]}}"#);
            }
            out.write(br#"{"schemaVersion":1,"result":{"ok":true}}"#)
        }
        fn destroy(self) -> Result<(), se_plugin_abi::SeStatus> {
            if self.mode.contains("slowdestroy") {
                std::thread::sleep(Duration::from_millis(700));
            }
            destroyed().lock().unwrap().push(self.mode);
            Ok(())
        }
        fn on_config_changed(&mut self, input: &[u8]) -> Result<(), se_plugin_abi::SeStatus> {
            if self.mode.contains("cancel") {
                std::thread::sleep(Duration::from_millis(200));
            }

            self.config = serde_json::from_slice::<serde_json::Value>(input).unwrap()["revision"]
                .as_u64()
                .unwrap();
            Ok(())
        }
    }
    fn manifest(id: &str, provides: &[&str], requires: &[&str]) -> Manifest {
        let services = |names: &[&str]| {
            names
                .iter()
                .map(|name| serde_json::json!({"name":name,"version":1}))
                .collect::<Vec<_>>()
        };
        serde_json::from_value(serde_json::json!({"schemaVersion":1,"id":id,"name":id,"version":"1.0.0","abi":{"min":1,"max":1},"services":{"provides":services(provides),"requires":services(requires)},"contributes":{"views":[{"id":"main","title":"Main","location":"panel"}],"commands":[{"id":"echo","title":"Echo"}],"configuration":{"title":"Settings","properties":{"enabled":{"type":"boolean","title":"Enabled","description":"","default":false}}}}})).unwrap()
    }
    #[test]
    fn web_invoke_accepts_w4_command_wire_key() {
        let params: WebInvokeParams =
            serde_json::from_value(json!({"command":"run","args":{"n":7}})).unwrap();
        assert_eq!(params.command_id, "run");
        assert_eq!(params.args, json!({"n":7}));
        let legacy: WebInvokeParams = serde_json::from_value(json!({"commandId":"run"})).unwrap();
        assert_eq!(legacy.command_id, "run");
        assert!(legacy.args.is_null());
        assert!(serde_json::from_value::<WebInvokeParams>(
            json!({"command":"run","pluginId":"org.other"})
        )
        .is_err());
        assert!(serde_json::from_value::<WebInvokeParams>(
            json!({"command":"run","commandId":"other"})
        )
        .is_err());
    }
    #[test]
    fn cancelled_web_requests_release_session_admission() {
        let root = tempfile::tempdir().unwrap();
        let services = Arc::new(crate::host_services::HostServices::new(
            root.path().into(),
            Uuid::new_v4().to_string(),
        ));
        let mut manifest = manifest("org.test.web", &[], &[]);
        manifest.contributes.views.push(serde_json::from_value(json!({"id":"panel","title":"Panel","location":"panel","kind":"web","entry":"index.html","allow":[]})).unwrap());
        let opened = services
            .web
            .lock()
            .unwrap()
            .open(
                &Uuid::new_v4().to_string(),
                "owner",
                &manifest,
                &Uuid::new_v4().to_string(),
                (
                    root.path().into(),
                    PackageDescriptor {
                        schema_version: 1,
                        native: BTreeMap::new(),
                        ui: "ui".into(),
                        files: vec![PackageFile {
                            path: "ui/index.html".into(),
                            size: 0,
                            sha256: "a".repeat(64),
                        }],
                    },
                ),
                &"a".repeat(64),
                "panel",
            )
            .unwrap();
        for _ in 0..MAX_WEB_PENDING_REQUESTS + 1 {
            let request_id = Uuid::new_v4().to_string();
            services
                .web
                .lock()
                .unwrap()
                .admit(&WebRequest {
                    session_id: opened.session_id.clone(),
                    request_id: request_id.clone(),
                    method: "theme.get".into(),
                    params: json!({}),
                })
                .unwrap();
            let guard = WebRequestGuard {
                services: services.clone(),
                session: opened.session_id.clone(),
                request: request_id,
            };
            drop(guard);
        }
    }
    fn fixture_api() -> se_plugin_abi::SePluginApiV1 {
        use se_plugin_abi::{export as e, *};
        SePluginApiV1 {
            abi_version: SEP_ABI_VERSION,
            struct_size: std::mem::size_of::<SePluginApiV1>() as u32,
            manifest: Some(e::manifest::<Fixture>),
            create: Some(e::create::<Fixture>),
            render_view: Some(e::render_view::<Fixture>),
            on_action: Some(e::on_action::<Fixture>),
            on_command: Some(e::on_command::<Fixture>),
            on_config_changed: Some(e::on_config_changed::<Fixture>),
            on_service_call: Some(e::on_service_call::<Fixture>),
            destroy: Some(e::destroy::<Fixture>),
        }
    }
    async fn install_fixture(host: &PluginHost, root: &Path, manifest: Manifest) -> PluginInfo {
        let extension = if cfg!(target_os = "macos") {
            "dylib"
        } else if cfg!(target_os = "windows") {
            "dll"
        } else {
            "so"
        };
        let source = root.join(format!("{}.{}", manifest.id, extension));
        loader::register_fixture(&source, manifest, fixture_api());
        host.install(InstallRequest {
            path: source.to_string_lossy().into_owned(),
        })
        .await
        .unwrap()
    }
    async fn enabled(host: &PluginHost, id: &str, value: bool) -> Result<PluginInfo, PluginError> {
        host.set_enabled(SetEnabledRequest {
            id: id.into(),
            enabled: value,
        })
        .await
    }
    #[tokio::test]
    async fn published_registry_changes_adopt_config_install_and_uninstall() {
        use se_foundation::durable_fs::CrashPoint;
        let root = tempfile::tempdir().unwrap();
        let host = PluginHost::open(root.path().into(), HostOptions::default())
            .await
            .unwrap();
        host.store.interrupt_next_write(CrashPoint::AfterReplace);
        install_fixture(&host, root.path(), manifest("test.published", &[], &[])).await;
        assert_eq!(host.store.read().unwrap().plugins.len(), 1);
        let before = host
            .get_config(GetConfigRequest {
                id: "test.published".into(),
            })
            .await
            .unwrap();
        host.store.interrupt_next_write(CrashPoint::AfterReplace);
        let saved = host
            .set_config(SetConfigRequest {
                id: "test.published".into(),
                expected_revision: before.revision,
                values: ConfigValues::from([("enabled".into(), ConfigValue::Boolean(true))]),
            })
            .await
            .unwrap();
        assert!(saved.revision > before.revision);
        assert_eq!(
            host.get_config(GetConfigRequest {
                id: "test.published".into()
            })
            .await
            .unwrap()
            .revision,
            saved.revision
        );
        assert_eq!(
            host.store.read().unwrap().plugins[0].config_revision,
            saved.revision
        );
        host.store.interrupt_next_write(CrashPoint::AfterReplace);
        host.uninstall(UninstallRequest {
            id: "test.published".into(),
        })
        .await
        .unwrap();
        assert!(host.list().await.unwrap().plugins.is_empty());
        assert!(host.store.read().unwrap().plugins.is_empty());
        host.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn unpublished_config_write_keeps_old_revision_and_values() {
        use se_foundation::durable_fs::CrashPoint;
        let root = tempfile::tempdir().unwrap();
        let host = PluginHost::open(root.path().into(), HostOptions::default())
            .await
            .unwrap();
        install_fixture(&host, root.path(), manifest("test.unpublished", &[], &[])).await;
        let before = host
            .get_config(GetConfigRequest {
                id: "test.unpublished".into(),
            })
            .await
            .unwrap();
        host.store.interrupt_next_write(CrashPoint::BeforeTempSync);
        assert_eq!(
            host.set_config(SetConfigRequest {
                id: "test.unpublished".into(),
                expected_revision: before.revision,
                values: ConfigValues::from([("enabled".into(), ConfigValue::Boolean(true))]),
            })
            .await
            .unwrap_err()
            .code,
            PLUGIN_IO
        );
        assert_eq!(
            host.get_config(GetConfigRequest {
                id: "test.unpublished".into()
            })
            .await
            .unwrap(),
            before
        );
        assert_eq!(
            host.store.read().unwrap().plugins[0].config_revision,
            before.revision
        );
        host.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn upgrade_manifest_fences_preupgrade_config_form() {
        let root = tempfile::tempdir().unwrap();
        let host = PluginHost::open(root.path().into(), HostOptions::default())
            .await
            .unwrap();
        install_fixture(&host, root.path(), manifest("test.upgrade", &[], &[])).await;
        let before = host
            .get_config(GetConfigRequest {
                id: "test.upgrade".into(),
            })
            .await
            .unwrap();
        let mut updated = manifest("test.upgrade", &[], &[]);
        updated.version = "2.0.0".into();
        updated
            .contributes
            .configuration
            .properties
            .get_mut("enabled")
            .unwrap()
            .default = ConfigValue::Boolean(true);
        let path = root.path().join("upgrade.dylib");
        loader::register_fixture(&path, updated, fixture_api());
        host.reload(ReloadRequest {
            id: "test.upgrade".into(),
            path: Some(path.to_string_lossy().into_owned()),
        })
        .await
        .unwrap();
        let after = host
            .get_config(GetConfigRequest {
                id: "test.upgrade".into(),
            })
            .await
            .unwrap();
        assert!(after.revision > before.revision);
        assert_eq!(
            after.values.get("enabled"),
            Some(&ConfigValue::Boolean(true))
        );
        assert_eq!(
            host.store.read().unwrap().plugins[0].config_revision,
            after.revision
        );
        assert_eq!(
            host.set_config(SetConfigRequest {
                id: "test.upgrade".into(),
                expected_revision: before.revision,
                values: before.values
            })
            .await
            .unwrap_err()
            .code,
            PLUGIN_CONFIG_CONFLICT
        );
        host.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn reinstall_after_restart_does_not_reuse_retired_config_revision() {
        let root = tempfile::tempdir().unwrap();
        let host = PluginHost::open(root.path().into(), HostOptions::default())
            .await
            .unwrap();
        install_fixture(&host, root.path(), manifest("test.reinstall", &[], &[])).await;
        let old = host
            .get_config(GetConfigRequest {
                id: "test.reinstall".into(),
            })
            .await
            .unwrap();
        let old = host
            .set_config(SetConfigRequest {
                id: "test.reinstall".into(),
                expected_revision: old.revision,
                values: ConfigValues::from([("enabled".into(), ConfigValue::Boolean(true))]),
            })
            .await
            .unwrap();
        host.uninstall(UninstallRequest {
            id: "test.reinstall".into(),
        })
        .await
        .unwrap();
        host.shutdown().await.unwrap();
        drop(host);
        let host = PluginHost::open(root.path().into(), HostOptions::default())
            .await
            .unwrap();
        install_fixture(&host, root.path(), manifest("test.reinstall", &[], &[])).await;
        let new = host
            .get_config(GetConfigRequest {
                id: "test.reinstall".into(),
            })
            .await
            .unwrap();
        assert!(new.revision > old.revision);
        assert_eq!(
            host.set_config(SetConfigRequest {
                id: "test.reinstall".into(),
                expected_revision: old.revision,
                values: old.values
            })
            .await
            .unwrap_err()
            .code,
            PLUGIN_CONFIG_CONFLICT
        );
        host.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn same_schema_reload_keeps_configuration_revision() {
        let root = tempfile::tempdir().unwrap();
        let host = PluginHost::open(root.path().into(), HostOptions::default())
            .await
            .unwrap();
        install_fixture(&host, root.path(), manifest("test.same-schema", &[], &[])).await;
        let before = host
            .get_config(GetConfigRequest {
                id: "test.same-schema".into(),
            })
            .await
            .unwrap();
        let mut changed = manifest("test.same-schema", &[], &[]);
        changed.version = "2.0.0".into();
        changed.contributes.commands.clear();
        let path = root.path().join("same_schema.dylib");
        loader::register_fixture(&path, changed, fixture_api());
        host.reload(ReloadRequest {
            id: "test.same-schema".into(),
            path: Some(path.to_string_lossy().into_owned()),
        })
        .await
        .unwrap();
        assert_eq!(
            host.get_config(GetConfigRequest {
                id: "test.same-schema".into()
            })
            .await
            .unwrap(),
            before
        );
        assert_eq!(
            host.store.read().unwrap().plugins[0].config_revision,
            before.revision
        );
        host.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn last_error_is_normalized_for_renderer_snapshots() {
        let root = tempfile::tempdir().unwrap();
        let host = PluginHost::open(root.path().into(), HostOptions::default())
            .await
            .unwrap();
        install_fixture(&host, root.path(), manifest("test.diagnostic", &[], &[])).await;
        host.state.lock().await.errors.insert(
            "test.diagnostic".into(),
            PluginError::new("bad", "😀".repeat(9000)),
        );
        let snapshot = host.list().await.unwrap();
        let diagnostic = snapshot.plugins[0].last_error.as_ref().unwrap();
        assert_eq!(diagnostic.code, PLUGIN_CALLBACK_ERROR);
        assert_eq!(diagnostic.message.encode_utf16().count(), 8192);
        host.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn dependency_arrival_and_withdrawal_release_every_lease() {
        let root = tempfile::tempdir().unwrap();
        let host = PluginHost::open(root.path().into(), HostOptions::default())
            .await
            .unwrap();
        install_fixture(
            &host,
            root.path(),
            manifest("test.consumer", &[], &["demo.service"]),
        )
        .await;
        assert_eq!(
            enabled(&host, "test.consumer", true).await.unwrap().state,
            PluginState::Pending
        );
        install_fixture(
            &host,
            root.path(),
            manifest("test.provider", &["demo.service"], &[]),
        )
        .await;
        assert_eq!(
            enabled(&host, "test.provider", true).await.unwrap().state,
            PluginState::Active
        );
        assert_eq!(host.diagnostics().active_instances, 2);
        assert_eq!(host.diagnostics().command_count, 2);
        assert_eq!(
            host.list().await.unwrap().plugins[0].state,
            PluginState::Active
        );
        enabled(&host, "test.provider", false).await.unwrap();
        assert_eq!(
            host.list().await.unwrap().plugins[0].state,
            PluginState::Pending
        );
        let d = host.diagnostics();
        assert_eq!(
            (
                d.active_instances,
                d.broker_registrations,
                d.view_count,
                d.command_count
            ),
            (0, 0, 0, 0)
        );
        assert_eq!(d.destroy_attempts, 2);
        host.shutdown().await.unwrap();
    }
    #[tokio::test]
    async fn duplicate_and_cycle_rejections_do_not_leave_instances() {
        for cycle in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let host = PluginHost::open(root.path().into(), HostOptions::default())
                .await
                .unwrap();
            install_fixture(
                &host,
                root.path(),
                manifest(
                    "test.first",
                    &["demo.one"],
                    if cycle { &["demo.two"] } else { &[] },
                ),
            )
            .await;
            install_fixture(
                &host,
                root.path(),
                manifest(
                    "test.second",
                    if cycle { &["demo.two"] } else { &["demo.one"] },
                    if cycle { &["demo.one"] } else { &[] },
                ),
            )
            .await;
            enabled(&host, "test.first", true).await.unwrap();
            let before = host.diagnostics();
            assert!(enabled(&host, "test.second", true).await.is_err());
            assert_eq!(host.diagnostics(), before);
            assert!(!host.list().await.unwrap().plugins[1].desired_enabled);
            host.shutdown().await.unwrap();
        }
    }
    #[tokio::test]
    async fn failed_post_create_activation_destroys_exactly_once() {
        for id in ["test.invalid", "test.panic"] {
            let root = tempfile::tempdir().unwrap();
            let host = PluginHost::open(root.path().into(), HostOptions::default())
                .await
                .unwrap();
            install_fixture(&host, root.path(), manifest(id, &[], &[])).await;
            assert!(enabled(&host, id, true).await.is_err());
            let d = host.diagnostics();
            assert_eq!(
                (
                    d.active_instances,
                    d.broker_registrations,
                    d.view_count,
                    d.command_count
                ),
                (0, 0, 0, 0)
            );
            assert_eq!(d.destroy_attempts, 1);
            host.shutdown().await.unwrap();
        }
    }
    #[tokio::test]
    async fn config_failure_restores_previous_disk_bytes_and_rebuilds_old_generation() {
        let root = tempfile::tempdir().unwrap();
        let host = PluginHost::open(root.path().into(), HostOptions::default())
            .await
            .unwrap();
        install_fixture(&host, root.path(), manifest("test.rebuild", &[], &[])).await;
        enabled(&host, "test.rebuild", true).await.unwrap();
        let before = std::fs::read(root.path().join("plugins/registry.json")).unwrap();
        let generation = host.list().await.unwrap().plugins[0].generation.clone();
        let request = SetConfigRequest {
            id: "test.rebuild".into(),
            expected_revision: 1,
            values: [("enabled".into(), ConfigValue::Boolean(true))]
                .into_iter()
                .collect(),
        };
        assert!(host.set_config(request).await.is_err());
        assert_eq!(
            std::fs::read(root.path().join("plugins/registry.json")).unwrap(),
            before
        );
        assert_eq!(
            host.get_config(GetConfigRequest {
                id: "test.rebuild".into()
            })
            .await
            .unwrap()
            .revision,
            1
        );
        assert_ne!(host.list().await.unwrap().plugins[0].generation, generation);
        assert_eq!(host.diagnostics().active_instances, 1);
        host.shutdown().await.unwrap();
    }
    #[tokio::test]
    async fn timeout_quarantines_without_destroy_and_keeps_inflight_memory_alive() {
        let root = tempfile::tempdir().unwrap();
        let options = HostOptions {
            callback_timeout: Duration::from_millis(300),
            ..HostOptions::default()
        };
        let host = PluginHost::open(root.path().into(), options).await.unwrap();
        install_fixture(&host, root.path(), manifest("test.timeout", &[], &[])).await;
        let mut restart = host.restart_requested();
        assert_eq!(
            enabled(&host, "test.timeout", true).await.unwrap_err().code,
            PLUGIN_TIMEOUT
        );
        restart.changed().await.unwrap();
        assert_eq!(restart.borrow().as_ref().unwrap().code, PLUGIN_TIMEOUT);
        let d = host.diagnostics();
        assert_eq!(d.active_instances, 0);
        assert_eq!(d.callbacks_in_flight, 1);
        assert_eq!(d.destroy_attempts, 0);
        tokio::time::sleep(Duration::from_millis(800)).await;
        assert_eq!(host.diagnostics().callbacks_in_flight, 0);
        host.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn aborted_config_waiter_still_commits_complete_transaction() {
        let root = tempfile::tempdir().unwrap();
        let host = PluginHost::open(root.path().into(), HostOptions::default())
            .await
            .unwrap();
        install_fixture(&host, root.path(), manifest("test.cancel", &[], &[])).await;
        enabled(&host, "test.cancel", true).await.unwrap();
        let clone = host.clone();
        let task = tokio::spawn(async move {
            clone
                .set_config(SetConfigRequest {
                    id: "test.cancel".into(),
                    expected_revision: 1,
                    values: [("enabled".into(), ConfigValue::Boolean(true))]
                        .into_iter()
                        .collect(),
                })
                .await
        });
        while host.diagnostics().callbacks_in_flight == 0 {
            tokio::task::yield_now().await;
        }
        task.abort();
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(
            host.get_config(GetConfigRequest {
                id: "test.cancel".into()
            })
            .await
            .unwrap()
            .revision,
            2
        );
        assert_eq!(
            host.get_view(GetViewRequest {
                id: "test.cancel".into(),
                view_id: "main".into()
            })
            .await
            .unwrap()
            .config_revision,
            2
        );
        host.shutdown().await.unwrap();
    }
    #[tokio::test]
    async fn startup_failure_remains_manageable() {
        let root = tempfile::tempdir().unwrap();
        let host = PluginHost::open(root.path().into(), HostOptions::default())
            .await
            .unwrap();
        install_fixture(
            &host,
            root.path(),
            manifest("test.invalid-reopen", &[], &[]),
        )
        .await;
        assert!(enabled(&host, "test.invalid-reopen", true).await.is_err());
        host.shutdown().await.unwrap();
        drop(host);
        tokio::task::yield_now().await;
        let reopened = PluginHost::open(root.path().into(), HostOptions::default())
            .await
            .unwrap();
        assert_eq!(
            reopened.list().await.unwrap().plugins[0].state,
            PluginState::Failed
        );
        enabled(&reopened, "test.invalid-reopen", false)
            .await
            .unwrap();
        reopened.shutdown().await.unwrap();
    }
    #[tokio::test]
    async fn dependent_destroy_precedes_provider_destroy() {
        let root = tempfile::tempdir().unwrap();
        let host = PluginHost::open(root.path().into(), HostOptions::default())
            .await
            .unwrap();
        install_fixture(
            &host,
            root.path(),
            manifest("test.order-provider", &["order.service"], &[]),
        )
        .await;
        install_fixture(
            &host,
            root.path(),
            manifest("test.order-consumer", &[], &["order.service"]),
        )
        .await;
        enabled(&host, "test.order-provider", true).await.unwrap();
        enabled(&host, "test.order-consumer", true).await.unwrap();
        enabled(&host, "test.order-provider", false).await.unwrap();
        let order: Vec<_> = destroyed()
            .lock()
            .unwrap()
            .iter()
            .filter(|id| id.starts_with("test.order-"))
            .cloned()
            .collect();
        assert_eq!(order, ["test.order-consumer", "test.order-provider"]);
        host.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn create_can_call_declared_provider_before_own_publication() {
        let root = tempfile::tempdir().unwrap();
        let host = PluginHost::open(root.path().into(), HostOptions::default())
            .await
            .unwrap();
        install_fixture(
            &host,
            root.path(),
            manifest("test.broker-provider", &["broker.service"], &[]),
        )
        .await;
        install_fixture(
            &host,
            root.path(),
            manifest("test.broker-consumer", &[], &["broker.service"]),
        )
        .await;
        enabled(&host, "test.broker-provider", true).await.unwrap();
        assert_eq!(
            enabled(&host, "test.broker-consumer", true)
                .await
                .unwrap()
                .state,
            PluginState::Active
        );
        host.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn failed_uninstall_persistence_restores_old_enabled_instances() {
        let root = tempfile::tempdir().unwrap();
        let host = PluginHost::open(root.path().into(), HostOptions::default())
            .await
            .unwrap();
        install_fixture(
            &host,
            root.path(),
            manifest("test.uninstall-provider", &["uninstall.service"], &[]),
        )
        .await;
        install_fixture(
            &host,
            root.path(),
            manifest("test.uninstall-consumer", &[], &["uninstall.service"]),
        )
        .await;
        enabled(&host, "test.uninstall-provider", true)
            .await
            .unwrap();
        enabled(&host, "test.uninstall-consumer", true)
            .await
            .unwrap();
        let registry = root.path().join("plugins/registry.json");
        let original = std::fs::read(&registry).unwrap();
        std::fs::write(&registry, b"corrupt").unwrap();
        assert_eq!(
            host.uninstall(UninstallRequest {
                id: "test.uninstall-provider".into()
            })
            .await
            .unwrap_err()
            .code,
            PLUGIN_STORE_CORRUPT
        );
        assert_eq!(host.diagnostics().active_instances, 2);
        assert_eq!(host.diagnostics().broker_registrations, 2);
        assert_eq!(std::fs::read(&registry).unwrap(), b"corrupt");
        std::fs::write(&registry, original).unwrap();
        host.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn cancelled_shutdown_completes_owned_cleanup() {
        let root = tempfile::tempdir().unwrap();
        let host = PluginHost::open(root.path().into(), HostOptions::default())
            .await
            .unwrap();
        install_fixture(
            &host,
            root.path(),
            manifest("test.slowdestroy-shutdown", &[], &[]),
        )
        .await;
        enabled(&host, "test.slowdestroy-shutdown", true)
            .await
            .unwrap();
        let clone = host.clone();
        let task = tokio::spawn(async move { clone.shutdown().await });
        while host.diagnostics().callbacks_in_flight == 0 {
            tokio::task::yield_now().await;
        }
        task.abort();
        tokio::time::sleep(Duration::from_millis(850)).await;
        assert_eq!(host.diagnostics().destroy_attempts, 1);
        assert_eq!(host.diagnostics().active_instances, 0);
        assert_eq!(host.diagnostics().broker_registrations, 0);
        assert!(!host.is_ready());
        assert_eq!(host.list().await.unwrap_err().code, PLUGIN_DISABLED);
    }
    #[tokio::test]
    async fn cordis_teardown_uses_explicit_worker_deadline_without_task_local() {
        let root = tempfile::tempdir().unwrap();
        let host = PluginHost::open(root.path().into(), HostOptions::default())
            .await
            .unwrap();
        install_fixture(
            &host,
            root.path(),
            manifest("test.explicit-deadline", &[], &[]),
        )
        .await;
        enabled(&host, "test.explicit-deadline", true)
            .await
            .unwrap();
        let worker = host.state.lock().await.active["test.explicit-deadline"]
            .worker
            .clone();
        worker.set_teardown_deadline(std::time::Instant::now() - Duration::from_millis(1));
        // A fresh detached task has no CONTROL_DEADLINE, like cordis cleanup.
        let clone = worker.clone();
        let result = tokio::spawn(async move { clone.call(CallKind::Destroy, Vec::new()).await })
            .await
            .unwrap();
        assert_eq!(result.unwrap_err().code, PLUGIN_BUSY);
        assert_eq!(host.diagnostics().destroy_attempts, 0);
        assert!(host.restart_requested().borrow().is_none());
        host.shutdown().await.unwrap();
        assert_eq!(host.diagnostics().destroy_attempts, 1);
    }
    #[tokio::test]
    async fn disconnected_worker_destroy_is_supervised_and_requests_restart() {
        let root = tempfile::tempdir().unwrap();
        let host = PluginHost::open(
            root.path().into(),
            HostOptions {
                callback_timeout: Duration::from_millis(300),
                ..HostOptions::default()
            },
        )
        .await
        .unwrap();
        install_fixture(
            &host,
            root.path(),
            manifest("test.slowdestroy-fallback", &[], &[]),
        )
        .await;
        let source = host.state.lock().await.registry.plugins[0]
            .source_path
            .clone();
        let (_, worker, _) = host
            .candidate(&source, "test.slowdestroy-fallback")
            .await
            .unwrap();
        worker.call(CallKind::Create,serde_json::to_vec(&serde_json::json!({"pluginId":"test.slowdestroy-fallback","config":{"revision":1}})).unwrap()).await.unwrap();
        let mut restart = host.restart_requested();
        drop(worker);
        tokio::time::timeout(Duration::from_millis(600), restart.changed())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(restart.borrow().as_ref().unwrap().code, PLUGIN_TIMEOUT);
        assert_eq!(host.diagnostics().destroy_attempts, 1);
        assert_eq!(host.diagnostics().callbacks_in_flight, 1);
        tokio::time::sleep(Duration::from_millis(800)).await;
        assert_eq!(host.diagnostics().callbacks_in_flight, 0);
        host.shutdown().await.unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn gc_skips_symlinked_package_levels_and_cleanup_roots() {
        use std::os::unix::fs::symlink;
        let hash = "a".repeat(64);
        for level in ["packages", "id", "version", "hash", "loaded", "boot"] {
            let root = tempfile::tempdir().unwrap();
            let outside = tempfile::tempdir().unwrap();
            let victim = match level {
                "packages" => outside.path().join("test.gc/1.0.0").join(&hash),
                "id" => outside.path().join("1.0.0").join(&hash),
                "version" => outside.path().join(&hash),
                "loaded" => outside.path().join(Uuid::new_v4().to_string()),
                _ => outside.path().join("preserve"),
            };
            std::fs::create_dir_all(&victim).unwrap();
            std::fs::write(victim.join("sentinel"), b"outside").unwrap();
            let link = match level {
                "packages" => root.path().join("packages"),
                "id" => root.path().join("packages/test.gc"),
                "version" => root.path().join("packages/test.gc/1.0.0"),
                "hash" => root.path().join("packages/test.gc/1.0.0").join(&hash),
                "loaded" => root.path().join("loaded"),
                _ => root.path().join("loaded").join(Uuid::new_v4().to_string()),
            };
            std::fs::create_dir_all(link.parent().unwrap()).unwrap();
            symlink(outside.path(), &link).unwrap();
            gc(
                root.path(),
                &Registry::default(),
                &Uuid::new_v4().to_string(),
            );
            assert_eq!(
                std::fs::read(victim.join("sentinel")).unwrap(),
                b"outside",
                "{level}"
            );
            assert!(std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink());
        }
    }

    #[test]
    fn gc_removes_only_recognized_unreferenced_directories() {
        let root = tempfile::tempdir().unwrap();
        let hash = "a".repeat(64);
        let valid = root.path().join("packages/test.gc/1.0.0").join(&hash);
        let preserved = [
            root.path().join("packages/INVALID/1.0.0").join(&hash),
            root.path().join("packages/test.gc/not-semver").join(&hash),
            root.path().join("packages/test.gc/1.0.0/invalid-hash"),
            root.path()
                .join("packages/test.gc/1.0.0")
                .join("B".repeat(64)),
            root.path().join("loaded/not-a-boot-id"),
        ];
        let boot = Uuid::new_v4().to_string();
        let current = root.path().join("loaded").join(&boot);
        let stale = root.path().join("loaded").join(Uuid::new_v4().to_string());
        let referenced = root
            .path()
            .join("packages/test.gc/1.0.0")
            .join("c".repeat(64));
        let mut registry = Registry::default();
        registry.plugins.push(RegistryEntry {
            package: None,
            id: "test.gc".to_owned(),
            manifest: manifest("test.gc", &[], &[]),
            source_path: root.path().join("source"),
            artifact_path: referenced.join("plugin.fixture"),
            sha256: "c".repeat(64),
            desired_enabled: false,
            config_revision: 1,
            config_overrides: BTreeMap::new(),
            quarantine: None,
        });
        for path in preserved
            .iter()
            .chain([&valid, &current, &stale, &referenced])
        {
            std::fs::create_dir_all(path).unwrap();
            std::fs::write(path.join("sentinel"), b"owned").unwrap();
        }
        gc(root.path(), &registry, &boot);
        assert!(!valid.exists());
        assert!(!stale.exists());
        assert!(referenced.join("sentinel").is_file());
        assert!(current.join("sentinel").is_file());
        for path in preserved {
            assert!(path.join("sentinel").is_file(), "{}", path.display());
        }
    }

    #[test]
    fn defaults_and_revision_limit() {
        let options = HostOptions::default();
        assert_eq!(options.callback_timeout, Duration::from_millis(2000));
        assert_eq!(options.queue_capacity, 32);
        assert_eq!(options.control_timeout, Duration::from_secs(20));
        assert!(next_revision(MAX_SAFE_INTEGER).is_err());
    }
}
