//! Close code 1008 revokes an entire iOS workspace; desktop-only policy must never use it.
use super::super::{
    auth::{IngressProvenance, RemoteRouteClass},
    upgraded_connections::UpgradedConnectionRegistry,
};
use super::*;
use axum::{body::to_bytes, middleware, routing::get, Router};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{client::IntoClientRequest, Message as WsMessage},
    MaybeTlsStream, WebSocketStream,
};
use tower::ServiceExt;

struct Fixture {
    authority: Arc<RemoteAccessAuthority>,
    service: Arc<DesktopService>,
    registry: Arc<UpgradedConnectionRegistry>,
    device_id: String,
    token: String,
    socket: WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
    server: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}
async fn fixture(control: bool) -> Fixture {
    let authority = Arc::new(RemoteAccessAuthority::for_tests("shutdown-test-legacy"));
    authority
        .set_public_origin(url::Url::parse("https://desktop.test").unwrap())
        .unwrap();
    let paired = authority
        .pair_device(crate::web::devices::PairRequest {
            code: authority.issue_pairing_code().unwrap().code,
            name: "Desktop shutdown test".into(),
            platform: "test".into(),
        })
        .unwrap();
    authority
        .set_device_scopes(
            &paired.device_id,
            vec!["screen.view".into(), "screen.control".into()],
        )
        .unwrap();
    let service = DesktopService::for_tests(true, control);
    let registry = Arc::new(UpgradedConnectionRegistry::new());
    let app = Router::new()
        .route("/remote/desktop/stream", get(stream))
        .layer(middleware::from_fn(crate::web::auth::capability_middleware))
        .layer(Extension(RemoteRouteClass::RemoteDesktop))
        .layer(Extension(IngressProvenance::PublicTunnel))
        .layer(Extension(authority.clone()))
        .layer(Extension(service.clone()))
        .layer(Extension(registry.clone()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    let mut request = format!("ws://{addr}/remote/desktop/stream")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert("origin", "https://desktop.test".parse().unwrap());
    let (mut socket, _) = connect_async(request).await.unwrap();
    socket
        .send(WsMessage::Text(
            json!({"id":"auth","type":"authenticate","payload":{"token":paired.device_token}})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    let authenticated = tokio::time::timeout(Duration::from_secs(2), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(
        matches!(authenticated,WsMessage::Text(text) if serde_json::from_str::<Value>(&text).unwrap()["success"]==true)
    );
    Fixture {
        authority,
        service,
        registry,
        device_id: paired.device_id,
        token: paired.device_token,
        socket,
        server,
    }
}
async fn assert_shutdown(fixture: &mut Fixture, code: &str, capability: &str, close: u16) {
    let error = tokio::time::timeout(Duration::from_secs(2), fixture.socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(
        matches!(error,WsMessage::Text(text) if serde_json::from_str::<Value>(&text).unwrap()==json!({"type":"error","code":code,"capability":capability})),
        "expected terminal {code}/{capability} before Close"
    );
    let closed = tokio::time::timeout(Duration::from_secs(2), fixture.socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(
        matches!(closed,WsMessage::Close(Some(frame)) if u16::from(frame.code)==close),
        "expected close {close}"
    );
    assert!(
        fixture.authority.verify_bearer(&fixture.token).is_ok(),
        "desktop policy must not revoke paired device"
    );
}

#[tokio::test]
async fn desktop_only_shutdowns_send_stable_error_then_normal_close_without_revocation() {
    for action in [
        "client_stop",
        "emergency",
        "view_off",
        "control_off",
        "scope_view",
        "cancel_only",
    ] {
        let mut f = fixture(action == "control_off").await;
        let (code, capability, close) = match action {
            "client_stop" => {
                f.socket
                    .send(WsMessage::Text(json!({"type":"stop"}).to_string().into()))
                    .await
                    .unwrap();
                ("DESKTOP_STOPPED", "screen.view", 1000)
            }
            "emergency" => {
                f.service.emergency_stop().unwrap();
                ("DESKTOP_STOPPED", "screen.view", 1000)
            }
            "view_off" => {
                f.service.set_settings(false, false).unwrap();
                ("DESKTOP_DISABLED", "screen.view", 1000)
            }
            "control_off" => {
                f.service.set_settings(true, false).unwrap();
                ("CONTROL_DISABLED", "screen.control", 1000)
            }
            "scope_view" => {
                f.authority
                    .set_device_scopes(&f.device_id, vec!["view".into()])
                    .unwrap();
                ("FORBIDDEN", "screen.view", 1000)
            }
            "cancel_only" => {
                assert_eq!(f.registry.cancel_device(&f.device_id), 1);
                ("DESKTOP_STOPPED", "screen.view", 1001)
            }
            _ => unreachable!(),
        };
        assert_shutdown(&mut f, code, capability, close).await;
    }
}
#[tokio::test]
async fn control_rejection_has_exact_capability_and_never_policy_revokes_workspace() {
    for scope_missing in [false, true] {
        let mut f = fixture(scope_missing).await;
        if scope_missing {
            f.authority
                .set_device_scopes(&f.device_id, vec!["screen.view".into()])
                .unwrap();
        }
        f.socket
            .send(WsMessage::Text(
                json!({"type":"text","text":"never injected"})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
        let (code, capability) = if scope_missing {
            ("FORBIDDEN", "screen.control")
        } else if screen::supported() {
            ("CONTROL_DISABLED", "screen.control")
        } else {
            ("SCREEN_UNSUPPORTED", "screen.view")
        };
        assert_shutdown(&mut f, code, capability, 1000).await;
    }
}
#[tokio::test]
async fn handler_scope_errors_include_capability_for_info_and_screenshot() {
    let f = fixture(false).await;
    let principal = f.authority.verify_bearer(&f.token).unwrap();
    f.authority
        .set_device_scopes(&f.device_id, vec!["view".into()])
        .unwrap();
    // Deliberately test the handler too: middleware and in-flight scope checks must agree.
    let app = Router::new()
        .route("/remote/desktop/info", get(info))
        .route("/remote/desktop/screenshot", get(screenshot))
        .layer(Extension(f.authority.clone()))
        .layer(Extension(principal))
        .layer(Extension(f.service.clone()));
    for path in [
        "/remote/desktop/info",
        "/remote/desktop/screenshot?display=1",
    ] {
        let response = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri(path)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let body: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
        assert_eq!(body["code"], "FORBIDDEN");
        assert_eq!(body["capability"], "screen.view");
    }
}
#[test]
fn only_real_device_revocation_can_select_policy_close() {
    for code in [
        "FORBIDDEN",
        "UNAUTHORIZED",
        "DESKTOP_STOPPED",
        "DESKTOP_DISABLED",
        "CONTROL_DISABLED",
        "SCREEN_UNSUPPORTED",
    ] {
        let close = DesktopClose::failure(AccessFailure::new(code, "screen.view"));
        assert_eq!(close.code, 1000);
        assert_eq!(close.error.unwrap()["code"], code);
    }
    assert_eq!(
        DesktopClose::failure(AccessFailure::new("DEVICE_REVOKED", "screen.view")).code,
        1008
    );
}
