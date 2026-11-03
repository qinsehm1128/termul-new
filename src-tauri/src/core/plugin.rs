//! Native plugin execution belongs exclusively to the `--plugin-core` process.
use super::ipc::*;
use super::transport::{
    connect_core, listen_core, CoreServerStream, CoreWriteHalf, ACCEPT_RETRY_BACKOFF,
};
use se_plugin_host::{HostOptions, PluginHost};
use se_plugin_protocol::*;
use serde::{de::DeserializeOwned, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::{broadcast, oneshot, watch, Mutex as AsyncMutex};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
fn business(code: &str, message: impl Into<String>) -> PluginError {
    PluginError {
        code: code.into(),
        message: message.into(),
    }
}
fn unavailable(message: impl Into<String>) -> PluginError {
    business(PLUGIN_CORE_UNAVAILABLE, message)
}
fn decode<T: DeserializeOwned>(value: Value) -> Result<T, PluginError> {
    serde_json::from_value(value).map_err(|e| business(PLUGIN_INVALID_REQUEST, e.to_string()))
}
fn encode<T: Serialize>(value: T) -> Result<Value, PluginError> {
    serde_json::to_value(value).map_err(|e| business(PLUGIN_INVALID_REQUEST, e.to_string()))
}

pub async fn run_plugin_core(profile_root: PathBuf) -> Result<(), CoreError> {
    let endpoint = CoreEndpoint::for_profile(&profile_root, CoreRole::PluginCore);
    prepare_runtime_dir(&endpoint)?;
    let mut listener = listen_core(&endpoint, CoreRole::PluginCore).await?;
    let opening = PluginHost::open(profile_root, HostOptions::default());
    tokio::pin!(opening);
    let mut initializing = tokio::task::JoinSet::new();
    let host = loop {
        tokio::select! {
            result = &mut opening => break result.map_err(|e| CoreError::Io(format!("{}: {}", e.code, e.message)))?,
            stream = listener.accept() => {
                if let Ok(mut stream) = stream {
                    initializing.spawn(async move {
                        let _ = tokio::time::timeout(Duration::from_secs(5), async {
                            let hello: CoreHello = read_json_frame(&mut stream).await?;
                            let ack = validate_hello_with_runtime(&hello, CoreRole::PluginCore, 0)?;
                            write_json_frame(&mut stream, &ack).await?;
                            let request: CoreRequest = read_json_frame(&mut stream).await?;
                            let response = if request.method == METHOD_HEALTH {
                                CoreResponse { id: request.id, result: Some(json!({
                                    "schemaVersion": 1, "bootId": uuid::Uuid::new_v4().to_string(), "pid": std::process::id(),
                                    "ready": false, "safeMode": false, "activeResources": 0,
                                })), error: None }
                            } else {
                                CoreResponse { id: request.id, result: None, error: Some(CoreErrorPayload {
                                    code: PLUGIN_CORE_UNAVAILABLE.into(), message: "Plugin Core is initializing".into(),
                                }) }
                            };
                            write_json_frame(&mut stream, &response).await
                        }).await;
                    });
                }
            }
            _ = initializing.join_next(), if !initializing.is_empty() => {}
        }
    };
    initializing.abort_all();
    // A hung native callback cannot be safely cancelled or unloaded in-process.
    let mut restart = host.restart_requested();
    tokio::spawn(async move {
        loop {
            if restart.borrow().is_some() {
                tokio::time::sleep(Duration::from_millis(100)).await;
                std::process::exit(1);
            }
            if restart.changed().await.is_err() {
                break;
            }
        }
    });
    serve_plugin_core_with_listener(host, endpoint, listener).await
}

#[cfg(test)]
async fn serve_plugin_core(host: Arc<PluginHost>, endpoint: CoreEndpoint) -> Result<(), CoreError> {
    prepare_runtime_dir(&endpoint)?;
    let listener = listen_core(&endpoint, CoreRole::PluginCore).await?;
    serve_plugin_core_with_listener(host, endpoint, listener).await
}

async fn serve_plugin_core_with_listener(
    host: Arc<PluginHost>,
    endpoint: CoreEndpoint,
    mut listener: super::transport::CoreListener,
) -> Result<(), CoreError> {
    let boot_id = host
        .list()
        .await
        .map_err(|e| CoreError::Io(e.message))?
        .boot_id;
    let (shutdown, mut stopped) = watch::channel(false);
    let mut sessions = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            _ = stopped.changed() => break,
            stream = listener.accept() => {
                let stream = match stream {
                    Ok(stream) => stream,
                    Err(error) => { eprintln!("[plugin-core] accept failed: {error}");
                        tokio::time::sleep(ACCEPT_RETRY_BACKOFF).await; continue; }
                };
                let host = host.clone();
                let boot_id = boot_id.clone();
                let shutdown = shutdown.clone();
                sessions.spawn(async move { let _ = serve_connection(stream, host, boot_id, shutdown).await; });
            }
            _ = sessions.join_next(), if !sessions.is_empty() => {}
        }
    }
    let _ = tokio::time::timeout(Duration::from_secs(3), host.shutdown()).await;
    sessions.abort_all();
    while sessions.join_next().await.is_some() {}
    drop(listener);
    remove_stale_socket(&endpoint)?;
    Ok(())
}

async fn serve_connection(
    mut stream: CoreServerStream,
    host: Arc<PluginHost>,
    boot_id: String,
    shutdown: watch::Sender<bool>,
) -> Result<(), CoreError> {
    let hello: CoreHello =
        tokio::time::timeout(Duration::from_secs(5), read_json_frame(&mut stream))
            .await
            .map_err(|_| CoreError::Io("plugin handshake timed out".into()))??;
    let ack = validate_hello_with_runtime(
        &hello,
        CoreRole::PluginCore,
        host.active_resources().min(u32::MAX as usize) as u32,
    )?;
    write_json_frame(&mut stream, &ack).await?;
    let (mut reader, writer) = stream.into_split();
    let writer = Arc::new(AsyncMutex::new(writer));
    // Framed reads are not cancellation safe: a dedicated task owns each complete read.
    let (incoming, mut received) = tokio::sync::mpsc::channel(32);
    let read_task = tokio::spawn(async move {
        loop {
            let frame = read_json_frame::<_, CoreRequest>(&mut reader).await;
            let failed = frame.is_err();
            if incoming.send(frame).await.is_err() || failed {
                break;
            }
        }
    });
    let mut events = host.subscribe();
    let mut requests = tokio::task::JoinSet::new();
    let outcome = loop {
        tokio::select! {
            request = received.recv() => {
                let request = match request { Some(Ok(r)) => r, Some(Err(e)) => break Err(e), None => break Ok(()) };
                let host = host.clone(); let boot_id = boot_id.clone();
                let writer = writer.clone(); let shutdown = shutdown.clone();
                requests.spawn(async move {
                    let is_shutdown = request.method == METHOD_SHUTDOWN || request.method == "shutdown";
                    let result = dispatch(&host, &boot_id, &request.method, request.params).await;
                    let stop = is_shutdown && result.is_ok();
                    let response = match result {
                        Ok(result) => CoreResponse { id: request.id, result: Some(result), error: None },
                        Err(error) => CoreResponse { id: request.id, result: None,
                            error: Some(CoreErrorPayload { code: error.code, message: error.message }) },
                    };
                    let _ = write_json_frame(&mut *writer.lock().await, &response).await;
                    if stop {
                        let _ = shutdown.send(true);
                    }
                });
            }
            event = events.recv() => {
                let event = match event {
                    Ok(event) => event,
                    Err(broadcast::error::RecvError::Lagged(_)) => break Err(CoreError::Io("plugin event stream lagged; reconnect for snapshot".into())),
                    Err(broadcast::error::RecvError::Closed) => break Ok(()),
                };
                let frame = CoreEvent { topic: TOPIC_PLUGIN_EVENT.into(),
                    payload: encode(event).map_err(|e| CoreError::InvalidFrame(e.message))? };
                if let Err(error) = write_json_frame(&mut *writer.lock().await, &frame).await { break Err(error); }
            }
            _ = requests.join_next(), if !requests.is_empty() => {}
        }
    };
    read_task.abort();
    requests.abort_all();
    outcome
}
async fn dispatch(
    host: &PluginHost,
    boot_id: &str,
    method: &str,
    params: Value,
) -> Result<Value, PluginError> {
    match method {
        METHOD_LIST => {
            let _: ListRequest = decode(params)?;
            encode(host.list().await?)
        }
        METHOD_INSTALL => encode(host.install(decode(params)?).await?),
        METHOD_SET_ENABLED => encode(host.set_enabled(decode(params)?).await?),
        METHOD_RELOAD => encode(host.reload(decode(params)?).await?),
        METHOD_UNINSTALL => encode(host.uninstall(decode(params)?).await?),
        METHOD_GET_VIEW => encode(host.get_view(decode(params)?).await?),
        METHOD_ACTION => encode(host.dispatch_action(decode(params)?).await?),
        METHOD_COMMAND => encode(host.execute_command(decode(params)?).await?),
        METHOD_GET_CONFIG => encode(host.get_config(decode(params)?).await?),
        METHOD_SET_CONFIG => encode(host.set_config(decode(params)?).await?),
        "health" => Ok(json!({ "ready": host.is_ready() })),
        "status" => Ok(json!({ "pid": std::process::id() })),
        METHOD_HEALTH => {
            let _: HealthRequest = decode(params)?;
            encode(HealthSnapshot {
                schema_version: SCHEMA_VERSION,
                boot_id: boot_id.into(),
                pid: std::process::id(),
                ready: host.is_ready(),
                safe_mode: host.safe_mode(),
                active_resources: host.active_resources() as u64,
            })
        }
        METHOD_SHUTDOWN => {
            let _: ShutdownRequest = decode(if params.is_null() { json!({}) } else { params })?;
            Ok(Value::Null)
        }
        _ => Err(business(
            PLUGIN_INVALID_REQUEST,
            format!("unknown plugin method: {method}"),
        )),
    }
}

#[derive(Debug, Clone)]
pub enum PluginClientEvent {
    Changed(PluginEvent),
    ConnectionChanged(PluginConnectionEvent),
}
type Pending = oneshot::Sender<Result<Value, PluginError>>;
struct ClientState {
    epoch: u64,
    next_id: u64,
    writer: Option<Arc<AsyncMutex<CoreWriteHalf>>>,
    reader: Option<tokio::task::JoinHandle<()>>,
    pending: HashMap<u64, Pending>,
    boot_id: Option<String>,
}
struct ClientInner {
    state: Mutex<ClientState>,
    reconnect: AsyncMutex<()>,
    events: broadcast::Sender<PluginClientEvent>,
}
// Cancellation during a framed write can leave the transport half-written. Invalidate
// only that connection epoch; an old request must never close its replacement.
struct PendingRequest {
    inner: Arc<ClientInner>,
    id: u64,
    epoch: u64,
    complete: bool,
}
impl Drop for PendingRequest {
    fn drop(&mut self) {
        let mut state = self.inner.state.lock().unwrap();
        if state.epoch != self.epoch {
            return;
        }
        state.pending.remove(&self.id);
        if !self.complete {
            if let Some(reader) = state.reader.take() {
                reader.abort();
            }
            disconnect_state(&mut state, &self.inner.events);
        }
    }
}
#[derive(Clone)]
pub struct PluginCoreClient {
    inner: Arc<ClientInner>,
}
impl PluginCoreClient {
    pub async fn connect(endpoint: &CoreEndpoint) -> Result<Self, CoreError> {
        let (events, _) = broadcast::channel(256);
        let client = Self {
            inner: Arc::new(ClientInner {
                state: Mutex::new(ClientState {
                    epoch: 0,
                    next_id: 1,
                    writer: None,
                    reader: None,
                    pending: HashMap::new(),
                    boot_id: None,
                }),
                reconnect: AsyncMutex::new(()),
                events,
            }),
        };
        client.reconnect(endpoint).await?;
        Ok(client)
    }
    pub fn is_connected(&self) -> bool {
        self.inner.state.lock().unwrap().writer.is_some()
    }
    pub fn boot_id(&self) -> Option<String> {
        self.inner.state.lock().unwrap().boot_id.clone()
    }
    pub fn subscribe_events(&self) -> broadcast::Receiver<PluginClientEvent> {
        self.inner.events.subscribe()
    }
    pub async fn disconnect(&self) {
        let mut state = self.inner.state.lock().unwrap();
        if let Some(reader) = state.reader.take() {
            reader.abort();
        }
        disconnect_state(&mut state, &self.inner.events);
    }
    pub async fn reconnect(&self, endpoint: &CoreEndpoint) -> Result<(), CoreError> {
        let _guard = self.inner.reconnect.lock().await;
        self.disconnect().await;
        let mut stream = connect_core(endpoint).await?;
        write_json_frame(
            &mut stream,
            &CoreHello {
                role: CoreRole::PluginCore,
                protocol_versions: vec![CURRENT_PROTOCOL_VERSION],
                client_name: "termul-gui-plugin".into(),
            },
        )
        .await?;
        let ack: CoreHelloAck =
            tokio::time::timeout(Duration::from_secs(5), read_json_frame(&mut stream))
                .await
                .map_err(|_| CoreError::Io("plugin handshake timed out".into()))??;
        if ack.role != CoreRole::PluginCore
            || ack.protocol_version != CURRENT_PROTOCOL_VERSION
            || !PLUGIN_CAPABILITIES
                .iter()
                .all(|cap| ack.capabilities.iter().any(|c| c == cap))
        {
            return Err(CoreError::InvalidRequest("incompatible Plugin Core".into()));
        }
        write_json_frame(
            &mut stream,
            &CoreRequest {
                id: 0,
                method: METHOD_HEALTH.into(),
                params: json!({}),
            },
        )
        .await?;
        let health: CoreResponse = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let frame: Value = read_json_frame(&mut stream).await?;
                if frame.get("topic").is_some() {
                    continue;
                }
                let response: CoreResponse = serde_json::from_value(frame)
                    .map_err(|e| CoreError::InvalidFrame(e.to_string()))?;
                if response.id != 0 {
                    return Err(CoreError::InvalidFrame(
                        "unexpected health response id".into(),
                    ));
                }
                return Ok::<_, CoreError>(response);
            }
        })
        .await
        .map_err(|_| CoreError::Io("plugin health timed out".into()))??;
        let health: HealthSnapshot = serde_json::from_value(
            health
                .result
                .ok_or_else(|| CoreError::Io("plugin health failed".into()))?,
        )
        .map_err(|e| CoreError::InvalidFrame(e.to_string()))?;
        if !health.ready {
            return Err(CoreError::Io("Plugin Core is initializing".into()));
        }
        let (mut reader, writer) = stream.into_split();
        let mut state = self.inner.state.lock().unwrap();
        state.epoch += 1;
        let epoch = state.epoch;
        state.boot_id = Some(health.boot_id.clone());
        state.writer = Some(Arc::new(AsyncMutex::new(writer)));
        let weak = Arc::downgrade(&self.inner);
        state.reader = Some(tokio::spawn(async move {
            loop {
                let frame = read_json_frame::<_, Value>(&mut reader).await;
                let Some(inner) = weak.upgrade() else {
                    break;
                };
                let mut state = inner.state.lock().unwrap();
                if state.epoch != epoch {
                    break;
                }
                match frame {
                    Ok(value) if value.get("topic").is_some() => {
                        if let Ok(event) = serde_json::from_value::<CoreEvent>(value) {
                            if event.topic == TOPIC_PLUGIN_EVENT {
                                if let Ok(payload) = serde_json::from_value(event.payload) {
                                    let _ = inner.events.send(PluginClientEvent::Changed(payload));
                                }
                            }
                        }
                    }
                    Ok(value) => {
                        if let Ok(response) = serde_json::from_value::<CoreResponse>(value) {
                            if let Some(sender) = state.pending.remove(&response.id) {
                                let result = match (response.result, response.error) {
                                    (_, Some(error)) => Err(business(&error.code, error.message)),
                                    (Some(value), None) => Ok(value),
                                    // JSON null maps to None for Option<Value>; shutdown intentionally returns null.
                                    (None, None) => Ok(Value::Null),
                                };
                                let _ = sender.send(result);
                            }
                        } else {
                            disconnect_state(&mut state, &inner.events);
                            break;
                        }
                    }
                    Err(_) => {
                        disconnect_state(&mut state, &inner.events);
                        break;
                    }
                }
            }
        }));
        let _ = self.inner.events.send(PluginClientEvent::ConnectionChanged(
            PluginConnectionEvent {
                state: ConnectionState::Connected,
                boot_id: Some(health.boot_id),
            },
        ));
        Ok(())
    }
    pub async fn request(&self, method: &str, params: Value) -> Result<Value, PluginError> {
        let (sender, receiver) = oneshot::channel();
        let (id, epoch, writer) = {
            let mut state = self.inner.state.lock().unwrap();
            let writer = state
                .writer
                .clone()
                .ok_or_else(|| unavailable("Plugin Core is disconnected"))?;
            let id = state.next_id;
            state.next_id += 1;
            let epoch = state.epoch;
            state.pending.insert(id, sender);
            (id, epoch, writer)
        };
        let mut pending = PendingRequest {
            inner: self.inner.clone(),
            id,
            epoch,
            complete: false,
        };
        // One deadline includes socket backpressure. Requests are never replayed on reconnect.
        let result = tokio::time::timeout(REQUEST_TIMEOUT, async {
            let mut receiver = receiver;
            {
            let sending = async {
                let mut writer = writer.lock().await;
                if self.inner.state.lock().unwrap().epoch != epoch {
                    return Err(unavailable("Plugin Core connection was replaced"));
                }
                write_json_frame(&mut *writer, &CoreRequest { id, method: method.into(), params })
                    .await.map_err(|e| unavailable(e.to_string()))
            };
            tokio::pin!(sending);
            tokio::select! {
                biased;
                result = &mut receiver => return result.map_err(|_| unavailable("Plugin Core disconnected"))?,
                result = &mut sending => result?,
            }
            }
            receiver.await.map_err(|_| unavailable("Plugin Core disconnected"))?
        })
        .await
        .unwrap_or_else(|_| Err(business(PLUGIN_TIMEOUT, "Plugin Core request timed out")));
        pending.complete = result
            .as_ref()
            .err()
            .is_none_or(|e| e.code != PLUGIN_TIMEOUT && e.code != PLUGIN_CORE_UNAVAILABLE);
        result
    }
}
fn disconnect_state(state: &mut ClientState, events: &broadcast::Sender<PluginClientEvent>) {
    let was_connected = state.writer.take().is_some();
    state.epoch += 1;
    for (_, sender) in state.pending.drain() {
        let _ = sender.send(Err(unavailable("Plugin Core disconnected")));
    }
    if was_connected {
        let _ = events.send(PluginClientEvent::ConnectionChanged(
            PluginConnectionEvent {
                state: ConnectionState::Disconnected,
                boot_id: state.boot_id.clone(),
            },
        ));
    }
}

/// Always managed by GUI, even when the external process could not be started.
#[derive(Clone)]
pub struct PluginServiceHandle {
    client: Arc<Mutex<Option<PluginCoreClient>>>,
    events: broadcast::Sender<PluginClientEvent>,
    connect_lock: Arc<AsyncMutex<()>>,
}
impl Default for PluginServiceHandle {
    fn default() -> Self {
        let (events, _) = broadcast::channel(256);
        Self {
            client: Arc::new(Mutex::new(None)),
            events,
            connect_lock: Arc::new(AsyncMutex::new(())),
        }
    }
}
impl PluginServiceHandle {
    pub fn client(&self) -> Option<PluginCoreClient> {
        self.client.lock().unwrap().clone()
    }
    pub fn is_connected(&self) -> bool {
        self.client().is_some_and(|c| c.is_connected())
    }
    pub fn boot_id(&self) -> Option<String> {
        self.client().and_then(|c| c.boot_id())
    }
    pub fn subscribe_events(&self) -> broadcast::Receiver<PluginClientEvent> {
        self.events.subscribe()
    }
    pub async fn disconnect(&self) {
        if let Some(client) = self.client() {
            client.disconnect().await;
        }
    }
    pub async fn connect(&self, endpoint: &CoreEndpoint) -> Result<(), CoreError> {
        let _guard = self.connect_lock.lock().await;
        if let Some(client) = self.client() {
            return client.reconnect(endpoint).await;
        }
        let client = PluginCoreClient::connect(endpoint).await?;
        let mut receiver = client.subscribe_events();
        let events = self.events.clone();
        let lag_client = client.clone();
        tokio::spawn(async move {
            loop {
                match receiver.recv().await {
                    Ok(event) => {
                        let _ = events.send(event);
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        let boot_id = lag_client.boot_id();
                        let _ = events.send(PluginClientEvent::ConnectionChanged(
                            PluginConnectionEvent {
                                state: ConnectionState::Disconnected,
                                boot_id: boot_id.clone(),
                            },
                        ));
                        if lag_client.is_connected() {
                            let _ = events.send(PluginClientEvent::ConnectionChanged(
                                PluginConnectionEvent {
                                    state: ConnectionState::Connected,
                                    boot_id,
                                },
                            ));
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });
        let boot_id = client.boot_id();
        *self.client.lock().unwrap() = Some(client);
        let _ = self.events.send(PluginClientEvent::ConnectionChanged(
            PluginConnectionEvent {
                state: ConnectionState::Connected,
                boot_id,
            },
        ));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn start(
        profile: &std::path::Path,
    ) -> (
        Arc<PluginHost>,
        CoreEndpoint,
        tokio::task::JoinHandle<Result<(), CoreError>>,
    ) {
        let host = PluginHost::open(profile.to_path_buf(), HostOptions::default())
            .await
            .unwrap();
        let endpoint = CoreEndpoint::for_profile(profile, CoreRole::PluginCore);
        let task = tokio::spawn(serve_plugin_core(host.clone(), endpoint.clone()));
        for _ in 0..100 {
            if connect_core(&endpoint).await.is_ok() {
                return (host, endpoint, task);
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("temp profile Plugin Core did not bind");
    }

    #[tokio::test]
    async fn unknown_method_returns_business_error_and_server_stays_alive() {
        let profile = tempfile::tempdir().unwrap();
        let (host, endpoint, task) = start(profile.path()).await;
        let client = PluginCoreClient::connect(&endpoint).await.unwrap();
        let error = client
            .request("plugin.no-such-method", json!({}))
            .await
            .unwrap_err();
        assert_eq!(error.code, PLUGIN_INVALID_REQUEST);
        let health: HealthSnapshot =
            serde_json::from_value(client.request(METHOD_HEALTH, json!({})).await.unwrap())
                .unwrap();
        assert!(health.ready);
        assert_eq!(health.boot_id, host.list().await.unwrap().boot_id);
        client.request(METHOD_SHUTDOWN, json!({})).await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        client.disconnect().await;
    }

    #[tokio::test]
    async fn disconnect_clears_pending_reconnect_rejects_old_boot_actions() {
        let profile = tempfile::tempdir().unwrap();
        let (host, endpoint, task) = start(profile.path()).await;
        let client = PluginCoreClient::connect(&endpoint).await.unwrap();
        let old_boot = client.boot_id().unwrap();
        let (sender, receiver) = oneshot::channel();
        client
            .inner
            .state
            .lock()
            .unwrap()
            .pending
            .insert(u64::MAX, sender);
        client.disconnect().await;
        assert_eq!(
            receiver.await.unwrap().unwrap_err().code,
            PLUGIN_CORE_UNAVAILABLE
        );
        assert!(client.inner.state.lock().unwrap().pending.is_empty());
        assert!(!client.is_connected());
        client.reconnect(&endpoint).await.unwrap();
        client.request(METHOD_SHUTDOWN, json!({})).await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        drop(host);
        let (new_host, endpoint, task) = start(profile.path()).await;
        client.reconnect(&endpoint).await.unwrap();
        assert_ne!(client.boot_id().unwrap(), old_boot);
        let error = client.request(METHOD_ACTION, json!({
            "schemaVersion": 1, "eventId": "30c93ed8-190a-4c69-9fae-e6502fb44975", "pluginId": "dev.test.plugin", "generation": "1ff91c70-3632-49d6-a66c-a22c4b3777d4",
            "viewId": "main", "viewRevision": 1, "nodeId": "button", "actionId": "click", "kind": "click", "value": null
        })).await.unwrap_err();
        // No plugin is installed in this profile. Real stale-generation validation is
        // exercised with the native fixture by the cross-process integration suite.
        assert_eq!(error.code, PLUGIN_NOT_FOUND);
        assert!(client.request(METHOD_LIST, json!({})).await.is_ok());
        client.request(METHOD_SHUTDOWN, json!({})).await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        drop(new_host);
    }

    #[tokio::test]
    async fn queued_old_epoch_request_is_rejected_without_waiting_for_writer() {
        let profile = tempfile::tempdir().unwrap();
        let (_, endpoint, task) = start(profile.path()).await;
        let client = PluginCoreClient::connect(&endpoint).await.unwrap();
        let writer = client.inner.state.lock().unwrap().writer.clone().unwrap();
        let blocked = writer.lock().await;
        let pending_client = client.clone();
        let request =
            tokio::spawn(async move { pending_client.request(METHOD_LIST, json!({})).await });
        while client.inner.state.lock().unwrap().pending.is_empty() {
            tokio::task::yield_now().await;
        }
        client.disconnect().await;
        let error = tokio::time::timeout(Duration::from_millis(200), request)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert_eq!(error.code, PLUGIN_CORE_UNAVAILABLE);
        assert!(client.inner.state.lock().unwrap().pending.is_empty());
        drop(blocked);
        client.reconnect(&endpoint).await.unwrap();
        client.request(METHOD_SHUTDOWN, Value::Null).await.unwrap();
        client.disconnect().await; // launcher closes immediately after acknowledgment
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn unavailable_core_does_not_prevent_service_creation() {
        let profile = tempfile::tempdir().unwrap();
        let service = PluginServiceHandle::default();
        let endpoint = CoreEndpoint::for_profile(profile.path(), CoreRole::PluginCore);
        assert!(service.connect(&endpoint).await.is_err());
        assert!(!service.is_connected());
        assert!(service.client().is_none());
    }
}
