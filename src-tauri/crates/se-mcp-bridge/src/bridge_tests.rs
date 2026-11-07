//! Wire security tests use temporary settings and ephemeral loopback listeners only.
use super::*;
use crate::{GatewayIdentity, GATEWAY_IDENTITY_PATH};
use axum::{
    extract::State,
    http::{HeaderMap, Method, StatusCode, Uri},
    response::{IntoResponse, Response},
    Json, Router,
};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Mutex as SyncMutex,
};

struct Probe {
    source: Source,
    identity: SyncMutex<Value>,
    identity_redirect: SyncMutex<Option<String>>,
    post_redirect: SyncMutex<Option<String>>,
    after_post_identity: SyncMutex<Option<Value>>,
    response_source: SyncMutex<Option<Source>>,
    response_version: SyncMutex<&'static str>,
    status: SyncMutex<StatusCode>,
    identities: AtomicUsize,
    posts: AtomicUsize,
    bearers: AtomicUsize,
    identity_bearers: AtomicUsize,
    controls: AtomicUsize,
    order: SyncMutex<Vec<&'static str>>,
}
impl Probe {
    fn new(source: Source) -> Arc<Self> {
        Arc::new(Self {
            source,
            identity: SyncMutex::new(serde_json::to_value(GatewayIdentity::new(source)).unwrap()),
            identity_redirect: SyncMutex::new(None),
            post_redirect: SyncMutex::new(None),
            after_post_identity: SyncMutex::new(None),
            response_source: SyncMutex::new(Some(source)),
            response_version: SyncMutex::new("1"),
            status: SyncMutex::new(StatusCode::OK),
            identities: AtomicUsize::new(0),
            posts: AtomicUsize::new(0),
            bearers: AtomicUsize::new(0),
            identity_bearers: AtomicUsize::new(0),
            controls: AtomicUsize::new(0),
            order: SyncMutex::new(Vec::new()),
        })
    }
    fn zero_credentials_and_posts(&self) {
        assert_eq!(self.posts.load(Ordering::SeqCst), 0);
        assert_eq!(self.bearers.load(Ordering::SeqCst), 0);
        assert_eq!(self.identity_bearers.load(Ordering::SeqCst), 0);
        assert_eq!(self.controls.load(Ordering::SeqCst), 0);
    }
}

async fn handle(
    State(probe): State<Arc<Probe>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: String,
) -> Response {
    let bearer = headers.contains_key("authorization");
    if bearer {
        probe.bearers.fetch_add(1, Ordering::SeqCst);
    }
    if uri.path() == GATEWAY_IDENTITY_PATH {
        probe.identities.fetch_add(1, Ordering::SeqCst);
        probe.order.lock().unwrap().push("identity");
        if bearer {
            probe.identity_bearers.fetch_add(1, Ordering::SeqCst);
        }
        if let Some(location) = probe.identity_redirect.lock().unwrap().clone() {
            return (StatusCode::TEMPORARY_REDIRECT, [("location", location)]).into_response();
        }
        return Json(probe.identity.lock().unwrap().clone()).into_response();
    }
    if uri.path() == "/control/status" {
        probe.controls.fetch_add(1, Ordering::SeqCst);
    }
    if method != Method::POST {
        return StatusCode::NOT_FOUND.into_response();
    }
    probe.posts.fetch_add(1, Ordering::SeqCst);
    probe.order.lock().unwrap().push("post");
    assert_eq!(headers.get(SOURCE_HEADER).unwrap(), probe.source.as_str());
    assert_eq!(headers.get(PROTOCOL_VERSION_HEADER).unwrap(), "1");
    assert!(bearer);
    if let Some(identity) = probe.after_post_identity.lock().unwrap().take() {
        *probe.identity.lock().unwrap() = identity;
    }
    let message: Value = serde_json::from_str(&body).unwrap();
    let mut response = if let Some(location) = probe.post_redirect.lock().unwrap().clone() {
        (StatusCode::TEMPORARY_REDIRECT, [("location", location)]).into_response()
    } else {
        (
            *probe.status.lock().unwrap(),
            Json(json!({"jsonrpc":"2.0","id":message["id"],"result":{"tools":[]}})),
        )
            .into_response()
    };
    if let Some(source) = *probe.response_source.lock().unwrap() {
        response
            .headers_mut()
            .insert(SOURCE_HEADER, source.as_str().parse().unwrap());
    }
    response.headers_mut().insert(
        PROTOCOL_VERSION_HEADER,
        probe.response_version.lock().unwrap().parse().unwrap(),
    );
    response
}

struct Server {
    port: u16,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn spawn_probe(probe: Arc<Probe>) -> Server {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let task = tokio::spawn(async move {
        axum::serve(listener, Router::new().fallback(handle).with_state(probe))
            .await
            .unwrap();
    });
    Server { port, task }
}
fn settings(path: &Path, port: u16, source: Source) {
    GatewaySettings {
        schema_version: crate::SCHEMA_VERSION,
        source,
        port,
        token: "TOKEN_SENTINEL_NEVER_LOG".into(),
        executable: None,
        profile_root: None,
        log_file: None,
    }
    .save(path)
    .unwrap();
}
fn bridge(
    path: PathBuf,
    source: Source,
    mode: Mode,
    url: Option<String>,
    autostart: bool,
) -> Gateway {
    Gateway::new(BridgeOptions {
        settings_path: path,
        source,
        mode,
        url,
        autostart,
    })
    .unwrap()
}
async fn request(bridge: &Gateway) -> Vec<Value> {
    bridge
        .forward(json!({"jsonrpc":"2.0","id":41,"method":"tools/list"}))
        .await
}

#[tokio::test]
async fn every_source_mode_and_direct_alias_preflights_before_each_post() {
    for source in [Source::Internal, Source::Aggregator] {
        for mode in [Mode::Grouped, Mode::Entry, Mode::Direct] {
            let probe = Probe::new(source);
            let server = spawn_probe(probe.clone()).await;
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("settings.json");
            settings(&path, server.port, source);
            let bridge = bridge(path, source, mode, None, false);
            for _ in 0..2 {
                assert!(request(&bridge).await[0].get("error").is_none());
            }
            assert_eq!(
                probe.order.lock().unwrap().as_slice(),
                ["identity", "post", "identity", "post"]
            );
            assert_eq!(probe.identity_bearers.load(Ordering::SeqCst), 0);
            if mode == Mode::Direct {
                let mut options = bridge.options;
                options.url = Some(format!("http://127.0.0.1:{}/mcp/direct", server.port));
                assert!(request(&Gateway::new(options).unwrap()).await[0]
                    .get("error")
                    .is_none());
            }
        }
    }
}

#[tokio::test]
async fn old_mixed_wrong_source_and_unknown_protocol_are_terminal_even_with_autostart() {
    for identity in [
        json!({}),
        json!({"running":true,"port":3290}),
        json!({"protocol":"se-mcp-gateway","protocolVersion":1,"source":"aggregator"}),
        json!({"protocol":"se-mcp-gateway","protocolVersion":2,"source":"internal"}),
        json!({"protocol":"mixed","protocolVersion":1,"source":"internal"}),
        json!({"protocol":"se-mcp-gateway","protocolVersion":1}),
    ] {
        let probe = Probe::new(Source::Internal);
        *probe.identity.lock().unwrap() = identity;
        let server = spawn_probe(probe.clone()).await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        settings(&path, server.port, Source::Internal);
        let bridge = bridge(path, Source::Internal, Mode::Grouped, None, true);
        let reply = request(&bridge).await;
        assert!(reply[0]["error"]["message"]
            .as_str()
            .unwrap()
            .starts_with("MCP_"));
        // Directly exercise the second pre-spawn check as well.
        assert_eq!(
            bridge.start().await.unwrap_err(),
            reply[0]["error"]["message"].as_str().unwrap()
        );
        probe.zero_credentials_and_posts();
    }
}

#[tokio::test]
async fn config_source_mismatch_never_contacts_a_listener() {
    let probe = Probe::new(Source::Internal);
    let server = spawn_probe(probe.clone()).await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    settings(&path, server.port, Source::Internal);
    let reply = request(&bridge(path, Source::Aggregator, Mode::Grouped, None, true)).await;
    assert_eq!(reply[0]["error"]["message"], "MCP_SETTINGS_SOURCE_MISMATCH");
    assert_eq!(probe.identities.load(Ordering::SeqCst), 0);
    probe.zero_credentials_and_posts();
}

#[tokio::test]
async fn url_overrides_never_move_credentials_or_select_another_mode() {
    let probe = Probe::new(Source::Internal);
    let server = spawn_probe(probe.clone()).await;
    let other_probe = Probe::new(Source::Internal);
    let other = spawn_probe(other_probe.clone()).await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    settings(&path, server.port, Source::Internal);
    for url in [
        format!("http://127.0.0.1:{}/mcp", other.port),
        format!("http://localhost:{}/mcp", server.port),
        format!("http://2130706433:{}/mcp", server.port),
        format!("http://@127.0.0.1:{}/mcp", server.port),
        format!("https://127.0.0.1:{}/mcp", server.port),
        format!("http://user:URL_SECRET@127.0.0.1:{}/mcp", server.port),
        format!("http://127.0.0.1:{}/mcp?URL_SECRET", server.port),
        format!("http://127.0.0.1:{}/mcp#URL_SECRET", server.port),
        format!("http://127.0.0.1:{}/mcp/entry", server.port),
        format!("http://127.0.0.1:{}/mcp/direct", server.port),
        "not a URL_SECRET".into(),
    ] {
        let reply = request(&bridge(
            path.clone(),
            Source::Internal,
            Mode::Grouped,
            Some(url),
            true,
        ))
        .await;
        assert_eq!(reply[0]["error"]["message"], "MCP_ENDPOINT_INVALID");
        assert!(!serde_json::to_string(&reply).unwrap().contains("SECRET"));
    }
    assert_eq!(probe.identities.load(Ordering::SeqCst), 0);
    probe.zero_credentials_and_posts();
    other_probe.zero_credentials_and_posts();
}

#[tokio::test]
async fn neither_identity_nor_authenticated_posts_follow_redirects() {
    let target_probe = Probe::new(Source::Internal);
    let target = spawn_probe(target_probe.clone()).await;
    let probe = Probe::new(Source::Internal);
    let server = spawn_probe(probe.clone()).await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    settings(&path, server.port, Source::Internal);
    *probe.identity_redirect.lock().unwrap() = Some(format!(
        "http://127.0.0.1:{}{}",
        target.port, GATEWAY_IDENTITY_PATH
    ));
    let reply = request(&bridge(
        path.clone(),
        Source::Internal,
        Mode::Grouped,
        None,
        true,
    ))
    .await;
    assert_eq!(reply[0]["error"]["message"], "MCP_IDENTITY_REJECTED");
    probe.zero_credentials_and_posts();
    *probe.identity_redirect.lock().unwrap() = None;
    *probe.post_redirect.lock().unwrap() = Some(format!("http://127.0.0.1:{}/mcp", target.port));
    let reply = request(&bridge(path, Source::Internal, Mode::Grouped, None, true)).await;
    assert_eq!(reply[0]["error"]["message"], "MCP_HTTP_REJECTED");
    target_probe.zero_credentials_and_posts();
    assert_eq!(target_probe.identities.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn unauthorized_retry_rechecks_identity_before_another_bearer() {
    let probe = Probe::new(Source::Internal);
    *probe.status.lock().unwrap() = StatusCode::UNAUTHORIZED;
    *probe.after_post_identity.lock().unwrap() =
        Some(serde_json::to_value(GatewayIdentity::new(Source::Aggregator)).unwrap());
    let server = spawn_probe(probe.clone()).await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    settings(&path, server.port, Source::Internal);
    let reply = request(&bridge(path, Source::Internal, Mode::Grouped, None, true)).await;
    assert_eq!(reply[0]["error"]["message"], "MCP_SOURCE_MISMATCH");
    assert_eq!(probe.posts.load(Ordering::SeqCst), 1);
    assert_eq!(probe.bearers.load(Ordering::SeqCst), 1);
    assert_eq!(
        probe.order.lock().unwrap().as_slice(),
        ["identity", "post", "identity"]
    );
    assert_eq!(probe.identity_bearers.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn response_source_and_protocol_mismatch_are_terminal() {
    for (source, version) in [(Source::Aggregator, "1"), (Source::Internal, "2")] {
        let probe = Probe::new(Source::Internal);
        *probe.response_source.lock().unwrap() = Some(source);
        *probe.response_version.lock().unwrap() = version;
        let server = spawn_probe(probe.clone()).await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        settings(&path, server.port, Source::Internal);
        let reply = request(&bridge(path, Source::Internal, Mode::Grouped, None, true)).await;
        assert_eq!(
            reply[0]["error"]["message"],
            "MCP_RESPONSE_IDENTITY_INVALID"
        );
        assert_eq!(probe.posts.load(Ordering::SeqCst), 1);
        assert_eq!(probe.identities.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn reread_source_mismatch_prevents_retry_and_repeated_401_is_bounded() {
    let probe = Probe::new(Source::Internal);
    *probe.status.lock().unwrap() = StatusCode::UNAUTHORIZED;
    let server = spawn_probe(probe.clone()).await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    settings(&path, server.port, Source::Internal);
    let cached = bridge(path.clone(), Source::Internal, Mode::Grouped, None, true);
    cached.settings(false).await.unwrap();
    settings(&path, server.port, Source::Aggregator);
    let reply = request(&cached).await;
    assert_eq!(reply[0]["error"]["message"], "MCP_SETTINGS_SOURCE_MISMATCH");
    assert_eq!(probe.posts.load(Ordering::SeqCst), 1);
    assert_eq!(
        request(&cached).await[0]["error"]["message"],
        "MCP_SETTINGS_SOURCE_MISMATCH"
    );
    assert_eq!(probe.posts.load(Ordering::SeqCst), 1);
    assert_eq!(probe.identity_bearers.load(Ordering::SeqCst), 0);
    settings(&path, server.port, Source::Internal);
    let reply = request(&bridge(path, Source::Internal, Mode::Grouped, None, true)).await;
    assert_eq!(reply[0]["error"]["message"], "MCP_AUTH_REJECTED");
    assert_eq!(probe.posts.load(Ordering::SeqCst), 3); // one cached attempt, two bounded attempts
    assert_eq!(probe.identities.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn missing_response_marker_is_terminal_including_on_401() {
    for status in [StatusCode::OK, StatusCode::UNAUTHORIZED] {
        let probe = Probe::new(Source::Internal);
        *probe.response_source.lock().unwrap() = None;
        *probe.status.lock().unwrap() = status;
        let server = spawn_probe(probe.clone()).await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        settings(&path, server.port, Source::Internal);
        let reply = request(&bridge(path, Source::Internal, Mode::Grouped, None, true)).await;
        assert_eq!(
            reply[0]["error"]["message"],
            "MCP_RESPONSE_IDENTITY_INVALID"
        );
        assert_eq!(probe.posts.load(Ordering::SeqCst), 1);
        assert_eq!(probe.identities.load(Ordering::SeqCst), 1);
        assert!(!serde_json::to_string(&reply)
            .unwrap()
            .contains("TOKEN_SENTINEL"));
    }
}
