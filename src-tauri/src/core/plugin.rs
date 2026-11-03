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
    serde_json::from_value(value).map_err(|_| invalid_request())
}
fn encode<T: Serialize>(value: T) -> Result<Value, PluginError> {
    serde_json::to_value(value).map_err(|_| invalid_request())
}

fn invalid_request() -> PluginError {
    business(PLUGIN_INVALID_REQUEST, "Invalid plugin request")
}
fn parse_host<T>(
    value: Value,
    parser: fn(&[u8]) -> Result<T, PluginError>,
) -> Result<T, PluginError> {
    let bytes = serde_json::to_vec(&value).map_err(|_| invalid_request())?;
    parser(&bytes).map_err(|_| invalid_request())
}

// Preserve duplicate-key admission information before serde_json::Value can erase it.
#[derive(Default)]
struct CheckedJson {
    value: Value,
    duplicate: bool,
}
impl<'de> serde::Deserialize<'de> for CheckedJson {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = CheckedJson;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("JSON")
            }
            fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<Self::Value, E> {
                Ok(CheckedJson {
                    value: v.into(),
                    duplicate: false,
                })
            }
            fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Self::Value, E> {
                Ok(CheckedJson {
                    value: v.into(),
                    duplicate: false,
                })
            }
            fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Self::Value, E> {
                Ok(CheckedJson {
                    value: v.into(),
                    duplicate: false,
                })
            }
            fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<Self::Value, E> {
                Ok(CheckedJson {
                    value: json!(v),
                    duplicate: false,
                })
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
                Ok(CheckedJson {
                    value: v.into(),
                    duplicate: false,
                })
            }
            fn visit_string<E: serde::de::Error>(self, v: String) -> Result<Self::Value, E> {
                Ok(CheckedJson {
                    value: v.into(),
                    duplicate: false,
                })
            }
            fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(CheckedJson {
                    value: Value::Null,
                    duplicate: false,
                })
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<Self::Value, A::Error> {
                let mut values = Vec::new();
                let mut duplicate = false;
                while let Some(child) = seq.next_element::<CheckedJson>()? {
                    duplicate |= child.duplicate;
                    values.push(child.value);
                }
                Ok(CheckedJson {
                    value: Value::Array(values),
                    duplicate,
                })
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<Self::Value, A::Error> {
                let mut values = serde_json::Map::new();
                let mut duplicate = false;
                while let Some((key, child)) = map.next_entry::<String, CheckedJson>()? {
                    duplicate |= child.duplicate || values.contains_key(&key);
                    values.insert(key, child.value);
                }
                Ok(CheckedJson {
                    value: Value::Object(values),
                    duplicate,
                })
            }
        }
        deserializer.deserialize_any(Visitor)
    }
}
#[derive(serde::Deserialize)]
struct PluginWireRequest {
    id: u64,
    method: String,
    #[serde(default)]
    params: CheckedJson,
}

// Keep the owner alive until all admitted requests have finished or been cancelled.
// Drop also runs when the connection task is aborted during Core shutdown.
struct ConnectionOwner {
    host: Arc<PluginHost>,
    id: String,
}
impl Drop for ConnectionOwner {
    fn drop(&mut self) {
        self.host.detach_host_context(&self.id);
    }
}
struct AbortReader(tokio::task::JoinHandle<()>);
impl Drop for AbortReader {
    fn drop(&mut self) {
        self.0.abort();
    }
}

macro_rules! host_rpc_methods {
    () => {
        pub async fn publish_host_context(
            &self,
            payload: HostContextPublishRequest,
        ) -> Result<HostContextAck, PluginError> {
            self.host_request(METHOD_HOST_CONTEXT_PUBLISH, payload)
                .await
        }
        pub async fn heartbeat_host_context(
            &self,
            payload: HostContextHeartbeatRequest,
        ) -> Result<HostContextAck, PluginError> {
            self.host_request(METHOD_HOST_CONTEXT_HEARTBEAT, payload)
                .await
        }
        pub async fn host_ui_snapshot(&self) -> Result<HostUiSnapshot, PluginError> {
            self.host_request(METHOD_HOST_UI_GET, HostUiGetRequest {})
                .await
        }
        pub async fn dismiss_host_ui(
            &self,
            payload: HostUiDismissRequest,
        ) -> Result<HostUiDismissResult, PluginError> {
            self.host_request(METHOD_HOST_UI_DISMISS, payload).await
        }
        pub async fn execute_contribution(
            &self,
            payload: ExecuteContributionRequest,
        ) -> Result<CommandReply, PluginError> {
            self.host_request(METHOD_CONTRIBUTION_EXECUTE, payload)
                .await
        }
    };
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
    stream: CoreServerStream,
    host: Arc<PluginHost>,
    boot_id: String,
    shutdown: watch::Sender<bool>,
) -> Result<(), CoreError> {
    let events = host.subscribe();
    serve_connection_with_events(stream, host, boot_id, shutdown, events).await
}
async fn serve_connection_with_events(
    mut stream: CoreServerStream,
    host: Arc<PluginHost>,
    boot_id: String,
    shutdown: watch::Sender<bool>,
    mut events: broadcast::Receiver<PluginEvent>,
) -> Result<(), CoreError> {
    let owner = Arc::new(ConnectionOwner {
        host: host.clone(),
        id: uuid::Uuid::new_v4().to_string(),
    });
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
    let read_task = AbortReader(tokio::spawn(async move {
        loop {
            let frame = read_json_frame::<_, PluginWireRequest>(&mut reader).await;
            let failed = frame.is_err();
            if incoming.send(frame).await.is_err() || failed {
                break;
            }
        }
    }));
    let mut requests = tokio::task::JoinSet::new();
    let outcome = loop {
        tokio::select! {
            request = received.recv() => {
                let request = match request { Some(Ok(r)) => r, Some(Err(e)) => break Err(e), None => break Ok(()) };
                let host = host.clone(); let boot_id = boot_id.clone();
                let writer = writer.clone(); let shutdown = shutdown.clone();
                let owner = owner.clone();
                requests.spawn(async move {
                    let is_shutdown = request.method == METHOD_SHUTDOWN || request.method == "shutdown";
                    let result = if request.params.duplicate { Err(invalid_request()) } else {
                        dispatch(&host, &boot_id, &owner.id, &request.method, request.params.value).await
                    };
                    let stop = is_shutdown && result.is_ok();
                    let response = match result {
                        Ok(result) => CoreResponse { id: request.id, result: Some(result), error: None },
                        Err(error) => CoreResponse { id: request.id, result: None,
                            error: Some(CoreErrorPayload { code: error.code, message: error.message }) },
                    };
                    let written = write_json_frame(&mut *writer.lock().await, &response).await;
                    if stop {
                        let _ = shutdown.send(true);
                    }
                    written
                });
            }
            event = events.recv() => {
                let event = match event {
                    Ok(event) => event,
                    Err(broadcast::error::RecvError::Lagged(_)) => break Err(CoreError::Io("plugin event stream lagged; reconnect for snapshot".into())),
                    Err(broadcast::error::RecvError::Closed) => break Ok(()),
                };
                let payload = match encode(event) {
                    Ok(payload) => payload,
                    Err(_) => break Err(CoreError::InvalidFrame("invalid plugin event".into())),
                };
                // Never await a socket write in the supervisor: EOF/read failure
                // must revoke the owner even when the peer stops reading.
                if requests.len() >= 32 { break Err(CoreError::Io("plugin event writer overflow".into())); }
                let frame = CoreEvent { topic: TOPIC_PLUGIN_EVENT.into(), payload };
                let writer = writer.clone();
                requests.spawn(async move { write_json_frame(&mut *writer.lock().await, &frame).await });
            }
            result = requests.join_next(), if !requests.is_empty() => {
                match result {
                    Some(Ok(Err(error))) => break Err(error),
                    Some(Err(_)) => break Err(CoreError::Io("plugin request task failed".into())),
                    _ => {}
                }
            }
        }
    };
    drop(read_task);
    requests.abort_all();
    while requests.join_next().await.is_some() {}
    drop(owner);
    if outcome.is_err() {
        log::warn!(target: "se_manager::core", "operation=plugin_connection_closed reason=transport_failure");
    }
    outcome
}
async fn dispatch(
    host: &PluginHost,
    boot_id: &str,
    owner: &str,
    method: &str,
    params: Value,
) -> Result<Value, PluginError> {
    match method {
        METHOD_HOST_CONTEXT_PUBLISH => encode(
            host.publish_host_context(
                owner,
                parse_host(params, parse_host_context_publish_request)?,
            )
            .await?,
        ),
        METHOD_HOST_CONTEXT_HEARTBEAT => {
            let request = parse_host(params, parse_host_context_heartbeat_request)?;
            encode(host.heartbeat_host_context(owner, &request.source_id)?)
        }
        METHOD_HOST_UI_GET => {
            let _ = parse_host(params, parse_host_ui_get_request)?;
            encode(host.host_ui_snapshot().await?)
        }
        METHOD_HOST_UI_DISMISS => encode(
            host.dismiss_host_ui(parse_host(params, parse_host_ui_dismiss_request)?)
                .await?,
        ),
        METHOD_CONTRIBUTION_EXECUTE => encode(
            host.execute_contribution(parse_host(params, parse_execute_contribution_request)?)
                .await?,
        ),
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
        _ => Err(invalid_request()),
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
    host_rpc_methods!();
    async fn host_request<T: DeserializeOwned, P: Serialize>(
        &self,
        method: &str,
        payload: P,
    ) -> Result<T, PluginError> {
        decode(self.request(method, encode(payload)?).await?)
    }

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
        // Existing read deadlines alone do not bound connect or backpressured
        // writes. Keep their five-second allowances within one total budget.
        tokio::time::timeout(Duration::from_secs(10), self.reconnect_inner(endpoint))
            .await
            .map_err(|_| CoreError::Io("plugin connection timed out".into()))?
    }
    async fn reconnect_inner(&self, endpoint: &CoreEndpoint) -> Result<(), CoreError> {
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
    host_rpc_methods!();
    async fn host_request<T: DeserializeOwned, P: Serialize>(
        &self,
        method: &str,
        payload: P,
    ) -> Result<T, PluginError> {
        let client = self
            .client()
            .ok_or_else(|| unavailable("Plugin Core is unavailable"))?;
        client.host_request(method, payload).await
    }

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
        tokio::time::timeout(Duration::from_secs(10), self.connect_inner(endpoint))
            .await
            .map_err(|_| CoreError::Io("plugin service connection timed out".into()))?
    }
    async fn connect_inner(&self, endpoint: &CoreEndpoint) -> Result<(), CoreError> {
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

    fn context_request(
        source_id: &str,
        source_sequence: u64,
        reset: bool,
    ) -> HostContextPublishRequest {
        HostContextPublishRequest {
            source_id: source_id.into(),
            source_sequence,
            reset,
            snapshot: HostContextState {
                active_project: None,
                project_ids: vec![],
                active_terminal_id: None,
                active_session_id: None,
                terminals: vec![],
                agent_sessions: vec![],
                focus: HostFocus {
                    text_input: false,
                    terminal: false,
                },
                truncated: false,
            },
            events: vec![],
        }
    }
    async fn publish_after_detach(client: &PluginCoreClient, source: &str) -> HostContextAck {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                match client
                    .publish_host_context(context_request(source, 1, true))
                    .await
                {
                    Ok(ack) => break ack,
                    Err(error) if error.code == PLUGIN_BUSY => {
                        tokio::time::sleep(Duration::from_millis(10)).await
                    }
                    Err(error) => panic!("unexpected source admission error: {}", error.code),
                }
            }
        })
        .await
        .expect("connection cleanup did not detach source")
    }
    async fn stop(client: &PluginCoreClient, task: tokio::task::JoinHandle<Result<(), CoreError>>) {
        client.request(METHOD_SHUTDOWN, json!({})).await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        client.disconnect().await;
    }

    #[tokio::test]
    async fn host_context_source_is_connection_scoped() {
        let profile = tempfile::tempdir().unwrap();
        let (_, endpoint, task) = start(profile.path()).await;
        let first = PluginCoreClient::connect(&endpoint).await.unwrap();
        let second = PluginCoreClient::connect(&endpoint).await.unwrap();
        let source = uuid::Uuid::new_v4().to_string();
        let initial = first
            .publish_host_context(context_request(&source, 1, true))
            .await
            .unwrap();
        assert_eq!(initial.source_sequence, 1);
        assert_eq!(
            second
                .publish_host_context(context_request(&source, 2, false))
                .await
                .unwrap_err()
                .code,
            PLUGIN_BUSY
        );
        assert_eq!(
            second
                .heartbeat_host_context(HostContextHeartbeatRequest {
                    source_id: source.clone()
                })
                .await
                .unwrap_err()
                .code,
            PLUGIN_BUSY
        );
        assert_eq!(
            second
                .publish_host_context(context_request(&uuid::Uuid::new_v4().to_string(), 1, true))
                .await
                .unwrap_err()
                .code,
            PLUGIN_BUSY
        );
        first.disconnect().await;
        // Reusing a sourceId on another connection still requires a fresh reset.
        publish_after_detach(&second, &source).await;
        let ack = second
            .heartbeat_host_context(HostContextHeartbeatRequest { source_id: source })
            .await
            .unwrap();
        assert_eq!(ack.source_sequence, 1);
        stop(&second, task).await;
    }

    #[tokio::test]
    async fn host_context_duplicate_sequence_is_not_redelivered() {
        let profile = tempfile::tempdir().unwrap();
        let (_, endpoint, task) = start(profile.path()).await;
        let client = PluginCoreClient::connect(&endpoint).await.unwrap();
        let source = uuid::Uuid::new_v4().to_string();
        client
            .publish_host_context(context_request(&source, 1, true))
            .await
            .unwrap();
        let next = context_request(&source, 2, false);
        let ack = client.publish_host_context(next.clone()).await.unwrap();
        assert_eq!(client.publish_host_context(next).await.unwrap(), ack);
        assert_eq!(
            client
                .heartbeat_host_context(HostContextHeartbeatRequest { source_id: source })
                .await
                .unwrap(),
            ack
        );
        stop(&client, task).await;
    }

    #[tokio::test]
    async fn host_context_gap_requires_reset() {
        let profile = tempfile::tempdir().unwrap();
        let (_, endpoint, task) = start(profile.path()).await;
        let client = PluginCoreClient::connect(&endpoint).await.unwrap();
        let source = uuid::Uuid::new_v4().to_string();
        client
            .publish_host_context(context_request(&source, 1, true))
            .await
            .unwrap();
        assert!(client
            .publish_host_context(context_request(&source, 3, false))
            .await
            .is_err());
        assert!(client
            .publish_host_context(context_request(&source, 2, false))
            .await
            .is_err());
        assert_eq!(
            client
                .publish_host_context(context_request(&source, 4, true))
                .await
                .unwrap()
                .source_sequence,
            4
        );
        stop(&client, task).await;
    }

    #[tokio::test]
    async fn host_context_heartbeat_expiry_requires_reset() {
        let profile = tempfile::tempdir().unwrap();
        let (_, endpoint, task) = start(profile.path()).await;
        let client = PluginCoreClient::connect(&endpoint).await.unwrap();
        let source = uuid::Uuid::new_v4().to_string();
        client
            .publish_host_context(context_request(&source, 1, true))
            .await
            .unwrap();
        // Lease uses wall time in PluginHost, not Tokio's pausable test clock.
        tokio::time::sleep(Duration::from_secs(16)).await;
        assert!(client
            .heartbeat_host_context(HostContextHeartbeatRequest {
                source_id: source.clone()
            })
            .await
            .is_err());
        assert!(client
            .publish_host_context(context_request(&source, 2, false))
            .await
            .is_err());
        assert_eq!(
            client
                .publish_host_context(context_request(&source, 3, true))
                .await
                .unwrap()
                .source_sequence,
            3
        );
        stop(&client, task).await;
    }

    #[tokio::test]
    async fn malformed_host_rpc_diagnostics_are_static() {
        let profile = tempfile::tempdir().unwrap();
        let (_, endpoint, task) = start(profile.path()).await;
        let client = PluginCoreClient::connect(&endpoint).await.unwrap();
        for method in [
            METHOD_HOST_CONTEXT_PUBLISH,
            METHOD_HOST_CONTEXT_HEARTBEAT,
            METHOD_HOST_UI_GET,
            METHOD_HOST_UI_DISMISS,
            METHOD_CONTRIBUTION_EXECUTE,
            "private-unknown-method",
        ] {
            let error = client
                .request(method, json!({"private-key":"private-value"}))
                .await
                .unwrap_err();
            assert_eq!(error.code, PLUGIN_INVALID_REQUEST);
            assert_eq!(error.message, "Invalid plugin request");
        }
        // GET and the two mutation methods are dispatched through the same transport.
        assert!(client.host_ui_snapshot().await.is_ok());
        let error = client
            .dismiss_host_ui(HostUiDismissRequest {
                plugin_id: "dev.test.plugin".into(),
                generation: uuid::Uuid::new_v4().to_string(),
                kind: HostUiKind::Notification,
                id: "notice".into(),
            })
            .await
            .unwrap_err();
        assert_ne!(error.code, PLUGIN_CORE_UNAVAILABLE);
        stop(&client, task).await;
    }

    #[tokio::test]
    async fn new_gui_rejects_missing_host_capability() {
        let profile = tempfile::tempdir().unwrap();
        let endpoint = CoreEndpoint::for_profile(profile.path(), CoreRole::PluginCore);
        prepare_runtime_dir(&endpoint).unwrap();
        let mut listener = listen_core(&endpoint, CoreRole::PluginCore).await.unwrap();
        let legacy = tokio::spawn(async move {
            let mut stream = listener.accept().await.unwrap();
            let hello: CoreHello = read_json_frame(&mut stream).await.unwrap();
            let mut ack = validate_hello_with_runtime(&hello, CoreRole::PluginCore, 0).unwrap();
            ack.capabilities
                .retain(|capability| capability != HOST_CAPABILITIES_V1);
            write_json_frame(&mut stream, &ack).await.unwrap();
        });
        let result = PluginCoreClient::connect(&endpoint).await;
        assert!(
            matches!(result, Err(CoreError::InvalidRequest(ref message)) if message == "incompatible Plugin Core")
        );
        legacy.await.unwrap();
    }

    // A fake Core isolates the forwarding contract from plugin-driven UI mutations.
    async fn ui_event_peer(
        endpoint: CoreEndpoint,
        boot: String,
        seq: u64,
    ) -> tokio::task::JoinHandle<()> {
        prepare_runtime_dir(&endpoint).unwrap();
        let mut listener = listen_core(&endpoint, CoreRole::PluginCore).await.unwrap();
        tokio::spawn(async move {
            let mut stream = listener.accept().await.unwrap();
            let hello: CoreHello = read_json_frame(&mut stream).await.unwrap();
            let ack = validate_hello_with_runtime(&hello, CoreRole::PluginCore, 0).unwrap();
            write_json_frame(&mut stream, &ack).await.unwrap();
            let request: CoreRequest = read_json_frame(&mut stream).await.unwrap();
            write_json_frame(&mut stream, &CoreResponse { id: request.id, result: Some(json!({"schemaVersion": 1, "bootId": boot, "pid": 1, "ready": true, "safeMode": false, "activeResources": 0})), error: None }).await.unwrap();
            // Wait for the test to attach its service bridge before sending the event.
            let request: CoreRequest = read_json_frame(&mut stream).await.unwrap();
            let event = PluginEvent {
                schema_version: 1,
                boot_id: boot,
                seq,
                kind: PluginEventType::HostUiChanged,
                plugin_id: None,
                generation: None,
                view_id: None,
                config_revision: None,
            };
            write_json_frame(
                &mut stream,
                &CoreEvent {
                    topic: TOPIC_PLUGIN_EVENT.into(),
                    payload: encode(event).unwrap(),
                },
            )
            .await
            .unwrap();
            write_json_frame(
                &mut stream,
                &CoreResponse {
                    id: request.id,
                    result: Some(json!({})),
                    error: None,
                },
            )
            .await
            .unwrap();
            let _ = read_json_frame::<_, CoreRequest>(&mut stream).await;
        })
    }
    #[tokio::test]
    async fn host_ui_changed_survives_service_reconnect() {
        let profile = tempfile::tempdir().unwrap();
        let endpoint = CoreEndpoint::for_profile(profile.path(), CoreRole::PluginCore);
        let service = PluginServiceHandle::default();
        let mut events = service.subscribe_events();
        for seq in [1, 2] {
            remove_stale_socket(&endpoint).unwrap();
            let boot = uuid::Uuid::new_v4().to_string();
            let peer = ui_event_peer(endpoint.clone(), boot.clone(), seq).await;
            service.connect(&endpoint).await.unwrap();
            service
                .client()
                .unwrap()
                .request("test.emit", json!({}))
                .await
                .unwrap();
            let event = tokio::time::timeout(Duration::from_secs(3), async {
                loop {
                    if let Ok(PluginClientEvent::Changed(event)) = events.recv().await {
                        break event;
                    }
                }
            })
            .await
            .unwrap();
            assert_eq!(event.kind, PluginEventType::HostUiChanged);
            assert_eq!(event.boot_id, boot);
            assert_eq!(event.seq, seq);
            service.client().unwrap().disconnect().await;
            tokio::time::timeout(Duration::from_secs(3), peer)
                .await
                .unwrap()
                .unwrap();
        }
    }

    #[cfg(unix)]
    async fn raw_connection(
        host: Arc<PluginHost>,
    ) -> (
        tokio::net::UnixStream,
        tokio::task::JoinHandle<Result<(), CoreError>>,
    ) {
        let (mut client, server) = tokio::net::UnixStream::pair().unwrap();
        let (shutdown, _) = watch::channel(false);
        let task = tokio::spawn(serve_connection(
            CoreServerStream::Unix(server),
            host,
            uuid::Uuid::new_v4().to_string(),
            shutdown,
        ));
        write_json_frame(
            &mut client,
            &CoreHello {
                role: CoreRole::PluginCore,
                protocol_versions: vec![CURRENT_PROTOCOL_VERSION],
                client_name: "p3-test".into(),
            },
        )
        .await
        .unwrap();
        let _: CoreHelloAck = read_json_frame(&mut client).await.unwrap();
        (client, task)
    }
    #[cfg(unix)]
    async fn raw_publish(stream: &mut tokio::net::UnixStream, source: &str) {
        write_json_frame(
            stream,
            &CoreRequest {
                id: 1,
                method: METHOD_HOST_CONTEXT_PUBLISH.into(),
                params: encode(context_request(source, 1, true)).unwrap(),
            },
        )
        .await
        .unwrap();
        let response: CoreResponse = read_json_frame(stream).await.unwrap();
        assert!(response.error.is_none());
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn host_context_event_lag_detaches_owner() {
        let profile = tempfile::tempdir().unwrap();
        let host = PluginHost::open(profile.path().to_path_buf(), HostOptions::default())
            .await
            .unwrap();
        let source = uuid::Uuid::new_v4().to_string();
        let (mut stream, server) = tokio::net::UnixStream::pair().unwrap();
        let (shutdown, _) = watch::channel(false);
        let (events, receiver) = broadcast::channel(1);
        let task = tokio::spawn(serve_connection_with_events(
            CoreServerStream::Unix(server),
            host.clone(),
            uuid::Uuid::new_v4().to_string(),
            shutdown,
            receiver,
        ));
        write_json_frame(
            &mut stream,
            &CoreHello {
                role: CoreRole::PluginCore,
                protocol_versions: vec![CURRENT_PROTOCOL_VERSION],
                client_name: "p3-test".into(),
            },
        )
        .await
        .unwrap();
        let _: CoreHelloAck = read_json_frame(&mut stream).await.unwrap();
        raw_publish(&mut stream, &source).await;
        let event = PluginEvent {
            schema_version: 1,
            boot_id: uuid::Uuid::new_v4().to_string(),
            seq: 1,
            kind: PluginEventType::HostUiChanged,
            plugin_id: None,
            generation: None,
            view_id: None,
            config_revision: None,
        };
        // No await between sends: the single-thread runtime deterministically lags.
        events.send(event.clone()).unwrap();
        events.send(event).unwrap();
        let result = tokio::time::timeout(Duration::from_secs(3), task)
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(result, Err(CoreError::Io(ref message)) if message == "plugin event stream lagged; reconnect for snapshot")
        );
        host.publish_host_context("replacement", context_request(&source, 1, true))
            .await
            .unwrap();
        host.shutdown().await.unwrap();
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn host_context_cancelled_connection_detaches_owner() {
        let profile = tempfile::tempdir().unwrap();
        let host = PluginHost::open(profile.path().to_path_buf(), HostOptions::default())
            .await
            .unwrap();
        let source = uuid::Uuid::new_v4().to_string();
        let (mut stream, task) = raw_connection(host.clone()).await;
        raw_publish(&mut stream, &source).await;
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        host.publish_host_context("replacement", context_request(&source, 1, true))
            .await
            .unwrap();
        host.shutdown().await.unwrap();
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn duplicate_host_rpc_fields_are_rejected_before_value_conversion() {
        use tokio::io::AsyncWriteExt;
        let profile = tempfile::tempdir().unwrap();
        let host = PluginHost::open(profile.path().to_path_buf(), HostOptions::default())
            .await
            .unwrap();
        let source = uuid::Uuid::new_v4().to_string();
        let (mut stream, task) = raw_connection(host.clone()).await;
        raw_publish(&mut stream, &source).await;
        let bytes = format!(r#"{{"id":2,"method":"plugins.host.context.heartbeat","params":{{"sourceId":"{source}","sourceId":"{source}"}}}}"#).into_bytes();
        stream
            .write_all(&(bytes.len() as u32).to_be_bytes())
            .await
            .unwrap();
        stream.write_all(&bytes).await.unwrap();
        let response: CoreResponse = read_json_frame(&mut stream).await.unwrap();
        let error = response.error.unwrap();
        assert_eq!(error.code, PLUGIN_INVALID_REQUEST);
        assert_eq!(error.message, "Invalid plugin request");
        drop(stream);
        assert!(tokio::time::timeout(Duration::from_secs(3), task)
            .await
            .unwrap()
            .unwrap()
            .is_err());
        host.shutdown().await.unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn host_context_backpressured_event_write_does_not_block_eof_cleanup() {
        use std::os::fd::AsRawFd;
        use tokio::io::AsyncWriteExt;
        let profile = tempfile::tempdir().unwrap();
        let host = PluginHost::open(profile.path().to_path_buf(), HostOptions::default())
            .await
            .unwrap();
        let source = uuid::Uuid::new_v4().to_string();
        let (mut stream, server) = tokio::net::UnixStream::pair().unwrap();
        let size: libc::c_int = 1024;
        assert_eq!(
            unsafe {
                libc::setsockopt(
                    server.as_raw_fd(),
                    libc::SOL_SOCKET,
                    libc::SO_SNDBUF,
                    (&size as *const libc::c_int).cast(),
                    std::mem::size_of_val(&size) as libc::socklen_t,
                )
            },
            0
        );
        let (shutdown, _) = watch::channel(false);
        let (events, receiver) = broadcast::channel(8);
        let task = tokio::spawn(serve_connection_with_events(
            CoreServerStream::Unix(server),
            host.clone(),
            uuid::Uuid::new_v4().to_string(),
            shutdown,
            receiver,
        ));
        write_json_frame(
            &mut stream,
            &CoreHello {
                role: CoreRole::PluginCore,
                protocol_versions: vec![CURRENT_PROTOCOL_VERSION],
                client_name: "p3-test".into(),
            },
        )
        .await
        .unwrap();
        let _: CoreHelloAck = read_json_frame(&mut stream).await.unwrap();
        raw_publish(&mut stream, &source).await;
        events
            .send(PluginEvent {
                schema_version: 1,
                boot_id: uuid::Uuid::new_v4().to_string(),
                seq: 1,
                kind: PluginEventType::HostUiChanged,
                plugin_id: Some("x".repeat(1024 * 1024)),
                generation: None,
                view_id: None,
                config_revision: None,
            })
            .unwrap();
        tokio::time::sleep(Duration::from_millis(30)).await;
        // Keep peer reads open but unread, while explicitly producing server EOF.
        stream.shutdown().await.unwrap();
        assert!(tokio::time::timeout(Duration::from_secs(3), task)
            .await
            .unwrap()
            .unwrap()
            .is_err());
        host.publish_host_context("replacement", context_request(&source, 1, true))
            .await
            .unwrap();
        host.shutdown().await.unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn host_context_read_failure_detaches_owner() {
        use tokio::io::AsyncWriteExt;
        let profile = tempfile::tempdir().unwrap();
        let host = PluginHost::open(profile.path().to_path_buf(), HostOptions::default())
            .await
            .unwrap();
        let source = uuid::Uuid::new_v4().to_string();
        let (mut stream, task) = raw_connection(host.clone()).await;
        raw_publish(&mut stream, &source).await;
        stream.write_all(&[0, 0, 0, 1, b'{']).await.unwrap();
        assert!(tokio::time::timeout(Duration::from_secs(3), task)
            .await
            .unwrap()
            .unwrap()
            .is_err());
        host.publish_host_context("replacement", context_request(&source, 1, true))
            .await
            .unwrap();
        host.shutdown().await.unwrap();
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn host_context_write_failure_detaches_owner() {
        use std::os::fd::AsRawFd;
        let profile = tempfile::tempdir().unwrap();
        let host = PluginHost::open(profile.path().to_path_buf(), HostOptions::default())
            .await
            .unwrap();
        let source = uuid::Uuid::new_v4().to_string();
        let (mut stream, server) = tokio::net::UnixStream::pair().unwrap();
        let server_fd = server.as_raw_fd();
        let (shutdown, _) = watch::channel(false);
        let task = tokio::spawn(serve_connection(
            CoreServerStream::Unix(server),
            host.clone(),
            uuid::Uuid::new_v4().to_string(),
            shutdown,
        ));
        write_json_frame(
            &mut stream,
            &CoreHello {
                role: CoreRole::PluginCore,
                protocol_versions: vec![CURRENT_PROTOCOL_VERSION],
                client_name: "p3-test".into(),
            },
        )
        .await
        .unwrap();
        let _: CoreHelloAck = read_json_frame(&mut stream).await.unwrap();
        raw_publish(&mut stream, &source).await;
        // The server still owns this fd and the client remains open: no EOF can
        // win the race. Disable only the server's writes after source admission.
        assert_eq!(unsafe { libc::shutdown(server_fd, libc::SHUT_WR) }, 0);
        write_json_frame(
            &mut stream,
            &CoreRequest {
                id: 2,
                method: METHOD_LIST.into(),
                params: json!({}),
            },
        )
        .await
        .unwrap();
        assert!(tokio::time::timeout(Duration::from_secs(3), task)
            .await
            .unwrap()
            .unwrap()
            .is_err());
        host.publish_host_context("replacement", context_request(&source, 1, true))
            .await
            .unwrap();
        host.shutdown().await.unwrap();
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

    #[tokio::test(start_paused = true)]
    async fn stalled_service_connect_is_bounded_and_stays_disconnected() {
        let profile = tempfile::tempdir().unwrap();
        let service = PluginServiceHandle::default();
        let endpoint = CoreEndpoint::for_profile(profile.path(), CoreRole::PluginCore);
        let _blocked = service.connect_lock.lock().await;
        let started = tokio::time::Instant::now();
        let result = service.connect(&endpoint).await;
        assert!(matches!(
            result,
            Err(CoreError::Io(message)) if message.contains("connection timed out")
        ));
        assert_eq!(started.elapsed(), Duration::from_secs(10));
        assert!(!service.is_connected());
        assert!(service.client().is_none());
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
