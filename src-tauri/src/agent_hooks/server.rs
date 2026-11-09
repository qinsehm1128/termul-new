//! A separate listener, deliberately not mounted in the remote web router.
use super::{normalize, publish, AgentKind};
use axum::{
    extract::{ConnectInfo, DefaultBodyLimit, Path, State},
    http::{HeaderMap, StatusCode},
    routing::post,
    Router,
};
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::{Arc, OnceLock},
};

#[derive(Clone)]
struct ReceiverState {
    secret: Arc<String>,
}
static STARTED: OnceLock<Result<(), String>> = OnceLock::new();

fn authorized(peer: SocketAddr, headers: &HeaderMap, secret: &str) -> bool {
    if peer.ip() != IpAddr::V4(Ipv4Addr::LOCALHOST) {
        return false;
    }
    let Some(value) = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
    else {
        return false;
    };
    // Constant-time compare after a public length check; never log either value.
    value.len() == secret.len()
        && value
            .bytes()
            .zip(secret.bytes())
            .fold(0u8, |a, (b, c)| a | (b ^ c))
            == 0
}
fn router(state: ReceiverState) -> Router {
    Router::new()
        .route("/agent-hooks/{agent}", post(receive))
        .layer(DefaultBodyLimit::max(1024 * 1024))
        .with_state(state)
}
async fn receive(
    State(state): State<ReceiverState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Path(agent): Path<AgentKind>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> StatusCode {
    if !authorized(peer, &headers, &state.secret) {
        log::warn!(target: "se_manager::agent_hooks", "operation=agent_hook_reject kind=authentication count=1");
        return StatusCode::UNAUTHORIZED;
    }
    let terminal = headers
        .get("x-se-terminal-id")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    let project = headers
        .get("x-se-project-id")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    let result = serde_json::from_slice(&body)
        .map_err(|_| "invalid_json")
        .and_then(|mut raw: serde_json::Value| {
            // qin-code has explicit hook environment metadata; preserve its raw
            // payload and use these headers only as missing-key fallbacks.
            if agent == AgentKind::QinCode {
                if let Some(object) = raw.as_object_mut() {
                    for (key, header) in [
                        ("session_id", "x-se-session-id"),
                        ("event", "x-se-hook-event"),
                    ] {
                        if !object.contains_key(key) {
                            if let Some(value) = headers
                                .get(header)
                                .and_then(|h| h.to_str().ok())
                                .filter(|s| !s.is_empty())
                            {
                                object.insert(key.into(), serde_json::Value::String(value.into()));
                            }
                        }
                    }
                }
            }
            normalize(agent, terminal, project, raw)
        });
    match result {
        Ok(event) => {
            publish(event);
            StatusCode::NO_CONTENT
        }
        Err(reason) => {
            log::warn!(target: "se_manager::agent_hooks", "operation=agent_hook_reject kind={} count=1", reason);
            StatusCode::BAD_REQUEST
        }
    }
}

fn bind_loopback() -> std::io::Result<std::net::TcpListener> {
    std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
}

/// Runtime metadata only; never touches agent configurations or scripts.
/// PTYs consult this file at spawn, so an existing terminal is never restarted.
pub fn start() -> Result<(), String> {
    STARTED.get_or_init(|| {
        let listener = bind_loopback().map_err(|e| e.to_string())?;
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        let secret = format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple());
        let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(std::path::PathBuf::from).ok_or("home directory unavailable")?;
        let directory = home.join(".se-manager/agent-hooks");
        std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
        let temp = directory.join(format!(".endpoint-{}", uuid::Uuid::new_v4()));
        let mut options = std::fs::OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)] { use std::os::unix::fs::OpenOptionsExt; options.mode(0o600); }
        use std::io::Write;
        let mut file = options.open(&temp).map_err(|e| e.to_string())?;
        write!(file, "SE_AGENT_HOOK_PORT={}\nSE_AGENT_HOOK_TOKEN={}\n", port, secret).map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        std::fs::rename(&temp, directory.join("endpoint.env")).map_err(|e| e.to_string())?;
        let app = router(ReceiverState { secret: Arc::new(secret) });
        tauri::async_runtime::spawn(async move {
            match tokio::net::TcpListener::from_std(listener) {
                Ok(listener) => if axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).await.is_err() { log::error!(target: "se_manager::agent_hooks", "operation=agent_hook_receiver kind=serve_failed count=1"); },
                Err(_) => log::error!(target: "se_manager::agent_hooks", "operation=agent_hook_receiver kind=listener_failed count=1"),
            }
        });
        log::info!(target: "se_manager::agent_hooks", "operation=agent_hook_receiver kind=started count=1");
        Ok(())
    }).clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;
    async fn request(peer: &str, token: &str, body: &str) -> StatusCode {
        let mut req = Request::builder()
            .method("POST")
            .uri("/agent-hooks/pi")
            .header("authorization", format!("Bearer {token}"))
            .header("x-se-terminal-id", "t")
            .header("x-se-project-id", "p")
            .body(Body::from(body.to_owned()))
            .unwrap();
        req.extensions_mut()
            .insert(ConnectInfo(peer.parse::<SocketAddr>().unwrap()));
        router(ReceiverState {
            secret: Arc::new("secret".into()),
        })
        .oneshot(req)
        .await
        .unwrap()
        .status()
    }
    #[test]
    fn listener_binds_only_ipv4_loopback_on_an_ephemeral_port() {
        let listener = bind_loopback().unwrap();
        let address = listener.local_addr().unwrap();
        assert_eq!(address.ip(), IpAddr::V4(Ipv4Addr::LOCALHOST));
        assert_ne!(address.port(), 0);
    }

    #[tokio::test]
    async fn qin_environment_metadata_is_a_fallback_not_a_payload_override() {
        for (payload, expected) in [
            (r#"{"tool_name":"read"}"#, "native-env"),
            (
                r#"{"session_id":"native-payload","event":"turn_end"}"#,
                "native-payload",
            ),
        ] {
            let mut req = Request::builder()
                .method("POST")
                .uri("/agent-hooks/qin-code")
                .header("authorization", "Bearer secret")
                .header("x-se-terminal-id", "qin-env-terminal")
                .header("x-se-project-id", "p")
                .header("x-se-hook-event", "post_tool")
                .header("x-se-session-id", "native-env")
                .body(Body::from(payload))
                .unwrap();
            req.extensions_mut()
                .insert(ConnectInfo("127.0.0.1:1234".parse::<SocketAddr>().unwrap()));
            assert_eq!(
                router(ReceiverState {
                    secret: Arc::new("secret".into())
                })
                .oneshot(req)
                .await
                .unwrap()
                .status(),
                StatusCode::NO_CONTENT
            );
            assert_eq!(
                super::super::latest("qin-env-terminal").unwrap().session_id,
                expected
            );
        }
    }

    #[tokio::test]
    async fn receiver_requires_secret_and_exact_ipv4_loopback() {
        let payload = r#"{"type":"session_start","session_id":"s"}"#;
        assert_eq!(
            request("127.0.0.1:1000", "secret", payload).await,
            StatusCode::NO_CONTENT
        );
        assert_eq!(
            request("127.0.0.1:1000", "bad", payload).await,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            request("192.168.1.1:1000", "secret", payload).await,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            request("[::1]:1000", "secret", payload).await,
            StatusCode::UNAUTHORIZED
        );
    }
    #[tokio::test]
    async fn receiver_rejects_malformed_missing_and_oversized_payloads() {
        for payload in ["{", "null", "{}", r#"{"type":"unknown","session_id":"s"}"#] {
            assert_eq!(
                request("127.0.0.1:1000", "secret", payload).await,
                StatusCode::BAD_REQUEST
            );
        }
        assert_eq!(
            request("127.0.0.1:1000", "secret", &"x".repeat(1024 * 1024 + 1)).await,
            StatusCode::PAYLOAD_TOO_LARGE
        );
    }
}
