use super::super::{
    auth::{IngressProvenance, RemoteRouteClass},
    upgraded_connections::UpgradedConnectionRegistry,
};
use super::*;
use axum::{body::to_bytes, middleware, routing::get, Router};
use tower::ServiceExt;

fn authority() -> Arc<RemoteAccessAuthority> {
    let authority = Arc::new(RemoteAccessAuthority::for_tests("desktop-test-legacy"));
    authority
        .set_public_origin(url::Url::parse("https://desktop.test").unwrap())
        .unwrap();
    authority
}
fn pair(authority: &RemoteAccessAuthority) -> crate::web::devices::PairResponse {
    authority
        .pair_device(crate::web::devices::PairRequest {
            code: authority.issue_pairing_code().unwrap().code,
            name: "Desktop test phone".into(),
            platform: "test".into(),
        })
        .unwrap()
}
fn desktop_app(authority: Arc<RemoteAccessAuthority>, service: Arc<DesktopService>) -> Router {
    Router::new()
        .route("/remote/desktop/info", get(info))
        .layer(middleware::from_fn(crate::web::auth::capability_middleware))
        .layer(Extension(RemoteRouteClass::RemoteDesktop))
        .layer(Extension(IngressProvenance::PublicTunnel))
        .layer(Extension(authority))
        .layer(Extension(service))
}
async fn request_info(app: Router, token: &str) -> (StatusCode, Value) {
    let response = app
        .oneshot(
            axum::http::Request::builder()
                .uri("/remote/desktop/info")
                .header("authorization", format!("Bearer {token}"))
                .extension(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 1))))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let value =
        serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
    (status, value)
}
#[tokio::test]
async fn info_route_view_disabled_and_missing_scope() {
    let authority = authority();
    let paired = pair(&authority);
    authority
        .set_device_scopes(&paired.device_id, vec!["screen.view".into()])
        .unwrap();
    let app = desktop_app(authority.clone(), DesktopService::for_tests(false, false));
    let (status, body) = request_info(app.clone(), &paired.device_token).await;
    if screen::supported() {
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["code"], "DESKTOP_DISABLED");
    } else {
        assert_eq!(body["supported"], false);
    }
    authority
        .set_device_scopes(&paired.device_id, vec!["view".into()])
        .unwrap();
    let (status, body) = request_info(app, &paired.device_token).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["code"], "FORBIDDEN");
}
#[test]
fn input_control_is_rejected_when_desktop_consent_is_off() {
    let authority = authority();
    let paired = pair(&authority);
    authority
        .set_device_scopes(
            &paired.device_id,
            vec!["screen.view".into(), "screen.control".into()],
        )
        .unwrap();
    let principal = authority.verify_bearer(&paired.device_token).unwrap();
    let service = DesktopService::for_tests(true, false);
    assert_eq!(
        authorize(&authority, &principal, &service, true),
        Err(if screen::supported() {
            "CONTROL_DISABLED"
        } else {
            "SCREEN_UNSUPPORTED"
        })
    );
    authority
        .set_device_scopes(&paired.device_id, vec!["screen.view".into()])
        .unwrap();
    assert_eq!(
        authorize(&authority, &principal, &service, true),
        Err("FORBIDDEN")
    );
}
#[tokio::test]
async fn revoke_closes_live_desktop_websocket_with_policy_code() {
    use tokio_tungstenite::{connect_async, tungstenite::Message as WsMessage};
    let authority = authority();
    let paired = pair(&authority);
    authority
        .set_device_scopes(&paired.device_id, vec!["screen.view".into()])
        .unwrap();
    let registry = Arc::new(UpgradedConnectionRegistry::new());
    authority
        .set_device_scopes(
            &paired.device_id,
            vec!["screen.view".into(), "screen.control".into()],
        )
        .unwrap();
    let app = Router::new()
        .route("/remote/desktop/stream", get(stream))
        .layer(middleware::from_fn(crate::web::auth::capability_middleware))
        .layer(Extension(RemoteRouteClass::RemoteDesktop))
        .layer(Extension(IngressProvenance::PublicTunnel))
        .layer(Extension(authority.clone()))
        .layer(Extension(DesktopService::for_tests(true, false)))
        .layer(Extension(registry.clone()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap()
    });
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
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
    let reply = tokio::time::timeout(Duration::from_secs(2), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(
        matches!(reply,WsMessage::Text(text) if serde_json::from_str::<Value>(&text).unwrap()["success"]==true)
    );
    socket
        .send(WsMessage::Text(
            json!({"type":"text","text":"never injected"})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    let rejected = tokio::time::timeout(Duration::from_secs(2), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let expected = if screen::supported() {
        "CONTROL_DISABLED"
    } else {
        "SCREEN_UNSUPPORTED"
    };
    assert!(
        matches!(rejected,WsMessage::Text(text) if serde_json::from_str::<Value>(&text).unwrap()["code"]==expected)
    );
    if screen::supported() {
        socket
            .send(WsMessage::Text(json!({"type":"stop"}).to_string().into()))
            .await
            .unwrap();
        let stopped = tokio::time::timeout(Duration::from_secs(2), socket.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(
            matches!(stopped,WsMessage::Text(text) if serde_json::from_str::<Value>(&text).unwrap()==json!({"type":"control_state","enabled":false,"active":false}))
        );
    }
    authority.revoke_device(&paired.device_id).unwrap();
    assert_eq!(registry.cancel_device(&paired.device_id), 1);
    let close = tokio::time::timeout(Duration::from_secs(2), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(matches!(close,WsMessage::Close(Some(frame)) if u16::from(frame.code)==1008));
    server.abort();
}
#[test]
fn upgrade_origin_and_bearer_match_terminal_authentication() {
    let authority = authority();
    let peer = IpAddr::from([192, 0, 2, 10]);
    assert!(upgrade_principal(&authority, peer, &HeaderMap::new()).is_err());
    let mut headers = HeaderMap::new();
    headers.insert("origin", "https://desktop.test".parse().unwrap());
    assert!(upgrade_principal(&authority, peer, &headers)
        .unwrap()
        .is_none());
    headers.insert("authorization", "Bearer invalid".parse().unwrap());
    assert!(upgrade_principal(&authority, peer, &headers).is_err());
    headers.remove("authorization");
    headers.insert("origin", "https://evil.example".parse().unwrap());
    assert!(upgrade_principal(&authority, peer, &headers).is_err());
}

#[test]
fn desktop_operator_requires_loopback_legacy_and_local_ingress() {
    let authority = authority();
    let legacy = authority.verify_bearer("desktop-test-legacy").unwrap();
    let paired = pair(&authority);
    let device = authority.verify_bearer(&paired.device_token).unwrap();
    let local = SocketAddr::from(([127, 0, 0, 1], 1));
    let lan = SocketAddr::from(([192, 0, 2, 4], 1));
    assert!(require_desktop_operator(
        Some(Extension(legacy.clone())),
        Some(Extension(IngressProvenance::LocalOperator)),
        local
    )
    .is_ok());
    assert_eq!(
        require_desktop_operator(
            Some(Extension(legacy.clone())),
            Some(Extension(IngressProvenance::PublicTunnel)),
            local
        ),
        Err("FORBIDDEN")
    );
    assert_eq!(
        require_desktop_operator(
            Some(Extension(legacy)),
            Some(Extension(IngressProvenance::LocalOperator)),
            lan
        ),
        Err("FORBIDDEN")
    );
    assert_eq!(
        require_desktop_operator(
            Some(Extension(device)),
            Some(Extension(IngressProvenance::LocalOperator)),
            local
        ),
        Err("FORBIDDEN")
    );
}

#[tokio::test]
async fn paired_device_cannot_read_or_set_local_consent_or_emergency_endpoint() {
    let authority = authority();
    let paired = pair(&authority);
    let device = authority.verify_bearer(&paired.device_token).unwrap();
    let app = Router::new()
        .route(
            "/api/remote-desktop/settings",
            get(get_settings).put(set_settings),
        )
        .route(
            "/api/remote-desktop/stop",
            axum::routing::post(emergency_stop),
        )
        .layer(Extension(device))
        .layer(Extension(IngressProvenance::LocalOperator));
    for (method, path, body) in [
        ("GET", "/api/remote-desktop/settings", ""),
        (
            "PUT",
            "/api/remote-desktop/settings",
            r#"{"viewEnabled":true,"controlEnabled":true}"#,
        ),
        ("POST", "/api/remote-desktop/stop", ""),
    ] {
        let response = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method(method)
                    .uri(path)
                    .header("content-type", "application/json")
                    .extension(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 1))))
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{method} {path}");
    }
}
