//! Stdio ↔ HTTP relay between one agent and the Se Manager MCP gateway.
//!
//! The agent speaks newline-delimited JSON-RPC on stdin/stdout. Every message
//! is POSTed to the gateway, which serves each request on its own (stateless
//! Streamable HTTP), so a gateway restart costs at most the requests in
//! flight: the next one simply lands on the new process.
//!
//! The port and token are read from the settings file, again whenever the
//! gateway refuses the connection or the token, so changing the port in Se
//! Manager needs no client change. When nothing listens, the relay starts the
//! gateway itself and waits for it.

use std::{
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};

use serde_json::{json, Value};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    sync::{mpsc, Mutex, RwLock},
    task::JoinSet,
};

use crate::settings::{GatewaySettings, Mode};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const START_TIMEOUT: Duration = Duration::from_secs(30);
const START_POLL: Duration = Duration::from_millis(200);
const ERROR_BODY_EXCERPT: usize = 300;

pub struct BridgeOptions {
    pub settings_path: PathBuf,
    pub mode: Mode,
    /// Full endpoint URL, bypassing the settings port (diagnostics only).
    pub url: Option<String>,
    /// Start the gateway when it is not running.
    pub autostart: bool,
}

/// Relay stdin to the gateway until stdin closes, then drain in-flight calls.
pub async fn run(options: BridgeOptions) -> std::io::Result<()> {
    let gateway = Arc::new(Gateway::new(options));
    let (replies, mut outbox) = mpsc::unbounded_channel::<Value>();
    let writer = tokio::spawn(async move {
        let mut stdout = tokio::io::stdout();
        while let Some(reply) = outbox.recv().await {
            let Ok(mut line) = serde_json::to_vec(&reply) else {
                continue;
            };
            line.push(b'\n');
            if stdout.write_all(&line).await.is_err() || stdout.flush().await.is_err() {
                break;
            }
        }
    });

    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut in_flight = JoinSet::new();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let message = match serde_json::from_str::<Value>(&line) {
            Ok(message) => message,
            Err(_) => {
                let _ = replies.send(error_reply(Value::Null, -32700, "Parse error".into()));
                continue;
            }
        };
        let gateway = Arc::clone(&gateway);
        let replies = replies.clone();
        in_flight.spawn(async move {
            for reply in gateway.forward(message).await {
                let _ = replies.send(reply);
            }
        });
        while in_flight.try_join_next().is_some() {}
    }
    while in_flight.join_next().await.is_some() {}
    drop(replies);
    let _ = writer.await;
    Ok(())
}

#[derive(Debug)]
enum PostError {
    /// Nothing accepted the connection: safe to retry, the request never left.
    Unreachable(String),
    Unauthorized,
    Failed(String),
}

pub(crate) struct Gateway {
    options: BridgeOptions,
    client: reqwest::Client,
    settings: RwLock<Option<GatewaySettings>>,
    /// Negotiated in `initialize`, sent on every later request.
    protocol_version: RwLock<Option<String>>,
    start_lock: Mutex<()>,
}

impl Gateway {
    pub(crate) fn new(options: BridgeOptions) -> Self {
        let client = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .build()
            .unwrap_or_default();
        Self {
            options,
            client,
            settings: RwLock::new(None),
            protocol_version: RwLock::new(None),
            start_lock: Mutex::new(()),
        }
    }

    /// Replies for one inbound message. A failure of a request becomes a
    /// JSON-RPC error on its id; a failed notification is dropped.
    pub(crate) async fn forward(&self, message: Value) -> Vec<Value> {
        let request_id = message.get("method").and(message.get("id")).cloned();
        let initialize = message.get("method").and_then(Value::as_str) == Some("initialize");
        match self.deliver(&message).await {
            Ok(replies) => {
                if initialize {
                    self.remember_protocol(&replies).await;
                }
                replies
            }
            Err(reason) => request_id
                .map(|id| vec![error_reply(id, -32603, reason)])
                .unwrap_or_default(),
        }
    }

    async fn deliver(&self, message: &Value) -> Result<Vec<Value>, String> {
        let settings = self.settings(false).await?;
        match self.post(&settings, message).await {
            Ok(replies) => Ok(replies),
            Err(PostError::Failed(reason)) => Err(reason),
            // The token or port may have changed since the file was read.
            Err(PostError::Unauthorized) => {
                let fresh = self.settings(true).await?;
                self.post(&fresh, message).await.map_err(describe)
            }
            Err(PostError::Unreachable(first)) => {
                let fresh = self.settings(true).await?;
                match self.post(&fresh, message).await {
                    Err(PostError::Unreachable(_)) if self.options.autostart => {
                        self.start(&fresh).await?;
                        self.post(&fresh, message).await.map_err(describe)
                    }
                    Err(PostError::Unreachable(_)) => Err(unreachable(&fresh, &first)),
                    other => other.map_err(describe),
                }
            }
        }
    }

    async fn settings(&self, reload: bool) -> Result<GatewaySettings, String> {
        if !reload {
            if let Some(settings) = self.settings.read().await.clone() {
                return Ok(settings);
            }
        }
        let path = &self.options.settings_path;
        let settings = GatewaySettings::load(path).map_err(|error| {
            format!(
                "cannot read Se Manager MCP gateway settings at {}: {error}. Open Se Manager once to create them.",
                path.display()
            )
        })?;
        *self.settings.write().await = Some(settings.clone());
        Ok(settings)
    }

    fn endpoint(&self, settings: &GatewaySettings) -> String {
        self.options
            .url
            .clone()
            .unwrap_or_else(|| format!("{}{}", settings.base_url(), self.options.mode.path()))
    }

    async fn post(
        &self,
        settings: &GatewaySettings,
        message: &Value,
    ) -> Result<Vec<Value>, PostError> {
        let mut request = self
            .client
            .post(self.endpoint(settings))
            .bearer_auth(&settings.token)
            .header(
                reqwest::header::ACCEPT,
                "application/json, text/event-stream",
            )
            .json(message);
        if let Some(version) = self.protocol_version.read().await.as_deref() {
            request = request.header("MCP-Protocol-Version", version);
        }
        let response = request.send().await.map_err(|error| {
            if error.is_connect() {
                PostError::Unreachable(error.to_string())
            } else {
                PostError::Failed(format!("Se Manager MCP gateway request failed: {error}"))
            }
        })?;
        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(PostError::Unauthorized);
        }
        let event_stream = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("text/event-stream"));
        let body = response.text().await.map_err(|error| {
            PostError::Failed(format!("reading the gateway reply failed: {error}"))
        })?;
        if !status.is_success() {
            let excerpt = body.chars().take(ERROR_BODY_EXCERPT).collect::<String>();
            return Err(PostError::Failed(format!(
                "Se Manager MCP gateway answered HTTP {status}: {excerpt}"
            )));
        }
        if body.trim().is_empty() {
            return Ok(Vec::new());
        }
        if event_stream {
            Ok(parse_event_stream(&body))
        } else {
            let value = serde_json::from_str::<Value>(&body).map_err(|error| {
                PostError::Failed(format!("the gateway reply is not JSON: {error}"))
            })?;
            Ok(match value {
                Value::Array(items) => items,
                other => vec![other],
            })
        }
    }

    async fn remember_protocol(&self, replies: &[Value]) {
        let version = replies.iter().find_map(|reply| {
            reply
                .pointer("/result/protocolVersion")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
        });
        if let Some(version) = version {
            *self.protocol_version.write().await = Some(version);
        }
    }

    async fn healthy(&self, settings: &GatewaySettings) -> bool {
        self.client
            .get(format!("{}/control/status", settings.base_url()))
            .bearer_auth(&settings.token)
            .send()
            .await
            .is_ok_and(|response| response.status().is_success())
    }

    /// Start the gateway and wait until it answers with this token. One
    /// start at a time: concurrent requests wait for the same start.
    async fn start(&self, settings: &GatewaySettings) -> Result<(), String> {
        let _starting = self.start_lock.lock().await;
        if self.healthy(settings).await {
            return Ok(());
        }
        let executable = gateway_executable(settings).ok_or_else(|| {
            "Se Manager MCP gateway is not running and Se Manager's location is unknown; open Se Manager once.".to_owned()
        })?;
        spawn_gateway(&executable, &self.options.settings_path, settings)
            .map_err(|error| format!("could not start the Se Manager MCP gateway: {error}"))?;
        let deadline = tokio::time::Instant::now() + START_TIMEOUT;
        while tokio::time::Instant::now() < deadline {
            if self.healthy(settings).await {
                return Ok(());
            }
            tokio::time::sleep(START_POLL).await;
        }
        Err(format!(
            "the Se Manager MCP gateway did not come up on port {} within {}s",
            settings.port,
            START_TIMEOUT.as_secs()
        ))
    }
}

fn describe(error: PostError) -> String {
    match error {
        PostError::Unreachable(reason) => format!("Se Manager MCP gateway is unreachable: {reason}"),
        PostError::Unauthorized => {
            "Se Manager MCP gateway rejected the token; reopen Se Manager to refresh the gateway settings".into()
        }
        PostError::Failed(reason) => reason,
    }
}

fn unreachable(settings: &GatewaySettings, reason: &str) -> String {
    format!(
        "Se Manager MCP gateway is not running on {} ({reason})",
        settings.base_url()
    )
}

/// The recorded executable, else a `se-manager` next to this client.
fn gateway_executable(settings: &GatewaySettings) -> Option<PathBuf> {
    if let Some(recorded) = settings.executable.as_ref().filter(|path| path.is_file()) {
        return Some(recorded.clone());
    }
    let sibling = std::env::current_exe()
        .ok()?
        .with_file_name(format!("se-manager{}", std::env::consts::EXE_SUFFIX));
    sibling.is_file().then_some(sibling)
}

/// Start `<executable> --mcp-core --config <settings>` detached from the
/// caller, so it keeps serving after the caller exits.
pub fn spawn_gateway(
    executable: &Path,
    settings_path: &Path,
    settings: &GatewaySettings,
) -> std::io::Result<()> {
    let mut command = std::process::Command::new(executable);
    command
        .arg("--mcp-core")
        .arg("--config")
        .arg(settings_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(profile_root) = &settings.profile_root {
        command.env("TERMUL_CORE_PROFILE_ROOT", profile_root);
    }
    if let Some(log_file) = &settings.log_file {
        command.env("TERMUL_CORE_LOG_FILE", log_file);
    }
    // Outlive this client: its own process group, no console.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
    }
    command.spawn().map(drop)
}

/// JSON-RPC messages carried in a `text/event-stream` body.
fn parse_event_stream(body: &str) -> Vec<Value> {
    let normalized = body.replace("\r\n", "\n");
    normalized
        .split("\n\n")
        .filter_map(|event| {
            let data = event
                .lines()
                .filter_map(|line| line.strip_prefix("data:"))
                .map(|data| data.strip_prefix(' ').unwrap_or(data))
                .collect::<Vec<_>>()
                .join("\n");
            serde_json::from_str::<Value>(&data).ok()
        })
        .collect()
}

fn error_reply(id: Value, code: i64, message: String) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        http::{HeaderMap, StatusCode},
        response::IntoResponse,
        routing::post,
        Json, Router,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn event_stream_bodies_yield_each_data_message() {
        let body = "id: 1\r\nevent: message\r\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\r\n\r\n: keepalive\n\ndata: {\"jsonrpc\":\"2.0\",\"method\":\"n\"}\n\n";
        let messages = parse_event_stream(body);
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["id"], 1);
        assert_eq!(messages[1]["method"], "n");
    }

    async fn spawn(router: Router) -> u16 {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        port
    }

    fn write_settings(dir: &Path, port: u16, token: &str) -> PathBuf {
        let path = dir.join("mcp-gateway.json");
        GatewaySettings {
            schema_version: 1,
            port,
            token: token.into(),
            executable: None,
            profile_root: None,
            log_file: None,
        }
        .save(&path)
        .unwrap();
        path
    }

    fn gateway(settings_path: PathBuf, mode: Mode) -> Gateway {
        Gateway::new(BridgeOptions {
            settings_path,
            mode,
            url: None,
            autostart: false,
        })
    }

    #[tokio::test]
    async fn relays_to_the_mode_path_with_the_token_and_negotiated_version() {
        let seen = Arc::new(std::sync::Mutex::new(Vec::<(String, Option<String>)>::new()));
        let record = Arc::clone(&seen);
        let router = Router::new().route(
            "/mcp/entry",
            post(move |headers: HeaderMap, Json(body): Json<Value>| {
                let record = Arc::clone(&record);
                async move {
                    if headers.get("authorization").and_then(|v| v.to_str().ok())
                        != Some("Bearer secret")
                    {
                        return StatusCode::UNAUTHORIZED.into_response();
                    }
                    let version = headers
                        .get("mcp-protocol-version")
                        .and_then(|v| v.to_str().ok())
                        .map(ToOwned::to_owned);
                    let method = body["method"].as_str().unwrap_or_default().to_owned();
                    record.lock().unwrap().push((method.clone(), version));
                    match method.as_str() {
                        "initialize" => Json(json!({
                            "jsonrpc": "2.0", "id": body["id"],
                            "result": { "protocolVersion": "2025-06-18" }
                        }))
                        .into_response(),
                        "notifications/initialized" => StatusCode::ACCEPTED.into_response(),
                        _ => (
                            [("content-type", "text/event-stream")],
                            format!(
                                "data: {}\n\n",
                                json!({ "jsonrpc": "2.0", "id": body["id"], "result": { "tools": [] } })
                            ),
                        )
                            .into_response(),
                    }
                }
            }),
        );
        let port = spawn(router).await;
        let dir = tempfile::tempdir().unwrap();
        let bridge = gateway(write_settings(dir.path(), port, "secret"), Mode::Entry);

        let init = bridge
            .forward(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}))
            .await;
        assert_eq!(init[0]["result"]["protocolVersion"], "2025-06-18");
        let accepted = bridge
            .forward(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
            .await;
        assert!(accepted.is_empty());
        let listed = bridge
            .forward(json!({"jsonrpc":"2.0","id":"t","method":"tools/list"}))
            .await;
        assert_eq!(listed[0]["id"], "t");
        assert!(listed[0]["result"]["tools"].is_array());

        let seen = seen.lock().unwrap().clone();
        assert_eq!(seen[0], ("initialize".into(), None));
        assert_eq!(seen[2], ("tools/list".into(), Some("2025-06-18".into())));
    }

    #[tokio::test]
    async fn a_changed_token_is_picked_up_from_the_settings_file() {
        let hits = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&hits);
        let router = Router::new().route(
            "/mcp",
            post(move |headers: HeaderMap, Json(body): Json<Value>| {
                let counter = Arc::clone(&counter);
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    if headers.get("authorization").and_then(|v| v.to_str().ok())
                        != Some("Bearer new")
                    {
                        return StatusCode::UNAUTHORIZED.into_response();
                    }
                    Json(json!({"jsonrpc":"2.0","id":body["id"],"result":{}})).into_response()
                }
            }),
        );
        let port = spawn(router).await;
        let dir = tempfile::tempdir().unwrap();
        let path = write_settings(dir.path(), port, "old");
        let bridge = gateway(path.clone(), Mode::Grouped);
        // Cache the old token, then rotate it on disk.
        bridge.settings(false).await.unwrap();
        write_settings(dir.path(), port, "new");

        let reply = bridge
            .forward(json!({"jsonrpc":"2.0","id":7,"method":"ping"}))
            .await;
        assert_eq!(reply[0]["id"], 7);
        assert!(reply[0].get("error").is_none());
        assert_eq!(hits.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn an_unreachable_gateway_fails_requests_but_not_notifications() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let dir = tempfile::tempdir().unwrap();
        let bridge = gateway(write_settings(dir.path(), port, "t"), Mode::Grouped);

        let reply = bridge
            .forward(json!({"jsonrpc":"2.0","id":3,"method":"tools/list"}))
            .await;
        assert_eq!(reply[0]["id"], 3);
        assert!(reply[0]["error"]["message"]
            .as_str()
            .unwrap()
            .contains("not running"));
        let silent = bridge
            .forward(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
            .await;
        assert!(silent.is_empty());
    }
}
