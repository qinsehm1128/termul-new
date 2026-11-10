use super::super::{
    auth::{IngressProvenance, RemoteRouteClass},
    upgraded_connections::UpgradedConnectionRegistry,
};
use super::*;
use axum::{body::to_bytes, middleware, routing::get, Router};
use tokio_tungstenite::{connect_async, tungstenite::Message as WsMessage};
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
    assert_eq!(body["capability"], "screen.view");
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
        authorize(&authority, &principal, &service, true).map_err(|error| error.code),
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
        authorize(&authority, &principal, &service, true).map_err(|error| error.code),
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
    authority.revoke_device(&paired.device_id).unwrap();
    assert_eq!(registry.cancel_device(&paired.device_id), 1);
    let revoked = tokio::time::timeout(Duration::from_secs(2), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(
        matches!(revoked,WsMessage::Text(text) if serde_json::from_str::<Value>(&text).unwrap()==json!({"type":"error","code":"DEVICE_REVOKED","capability":"screen.view"}))
    );
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

fn stream_app(authority: Arc<RemoteAccessAuthority>, service: Arc<DesktopService>) -> Router {
    Router::new()
        .route("/remote/desktop/info", get(info))
        .route("/remote/desktop/stream", get(stream))
        .layer(middleware::from_fn(crate::web::auth::capability_middleware))
        .layer(Extension(RemoteRouteClass::RemoteDesktop))
        .layer(Extension(IngressProvenance::PublicTunnel))
        .layer(Extension(authority))
        .layer(Extension(service))
}

type TestSocket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn next_message(socket: &mut TestSocket) -> WsMessage {
    tokio::time::timeout(Duration::from_secs(10), socket.next())
        .await
        .expect("timed out waiting for a websocket message")
        .expect("websocket closed")
        .expect("websocket error")
}

async fn next_meta(socket: &mut TestSocket) -> Value {
    loop {
        if let WsMessage::Text(text) = next_message(socket).await {
            let value: Value = serde_json::from_str(&text).unwrap();
            if value["type"] == "meta" {
                return value;
            }
        }
    }
}

async fn next_binary(socket: &mut TestSocket) -> Vec<u8> {
    loop {
        if let WsMessage::Binary(bytes) = next_message(socket).await {
            return bytes.to_vec();
        }
    }
}

async fn connect_stream(addr: SocketAddr, token: &str) -> TestSocket {
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
            json!({"id":"auth","type":"authenticate","payload":{"token":token}})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    loop {
        if let WsMessage::Text(text) = next_message(&mut socket).await {
            let value: Value = serde_json::from_str(&text).unwrap();
            if value["success"] == true {
                break;
            }
            if value["type"] == "error" {
                panic!("authentication failed: {value}");
            }
        }
    }
    socket
}

fn fake_capture() -> Box<dyn FnMut(&str, u32) -> Result<image::RgbaImage, screen::ScreenError> + Send>
{
    Box::new(|_, _| Ok(image::RgbaImage::from_pixel(320, 240, image::Rgba([1, 2, 3, 255]))))
}

/// `info` always advertises `jpeg`, and `h264` exactly when a session is usable.
#[tokio::test]
async fn info_advertises_codecs_array() {
    let authority = authority();
    let paired = pair(&authority);
    authority
        .set_device_scopes(&paired.device_id, vec!["screen.view".into()])
        .unwrap();
    let app = desktop_app(authority, DesktopService::for_tests(true, false));
    let (status, body) = request_info(app, &paired.device_token).await;
    assert_eq!(status, StatusCode::OK);
    let codecs = body["codecs"].as_array().expect("codecs must be an array");
    assert!(
        codecs.iter().any(|codec| codec == "jpeg"),
        "jpeg is always advertised"
    );
    assert_eq!(
        codecs.iter().any(|codec| codec == "h264"),
        screen::available_codecs().contains(&"h264")
    );
}

/// RV-010: legacy start → JPEG @3fps, unusable h264 → JPEG @3fps, and an h264
/// start round trip that publishes a real IDR (fake capture, no TCC prompt).
#[tokio::test]
async fn stream_start_negotiates_the_frozen_meta() {
    if !screen::supported() {
        eprintln!("skipping: desktop capture is unavailable in this test session");
        return;
    }
    let authority = authority();
    let paired = pair(&authority);
    authority
        .set_device_scopes(&paired.device_id, vec!["screen.view".into()])
        .unwrap();
    let service = DesktopService::for_tests(true, false);
    let app = stream_app(authority.clone(), service);
    let (status, body) = request_info(app.clone(), &paired.device_token).await;
    assert_eq!(status, StatusCode::OK);
    let Some(display) = body["displays"]
        .as_array()
        .and_then(|list| list.first())
        .and_then(|display| display["id"].as_str())
        .map(str::to_owned)
    else {
        eprintln!("skipping: this host has no display");
        return;
    };
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
    let mut socket = connect_stream(addr, &paired.device_token).await;

    // 1. Legacy payload (no codec/bitrate): JPEG at the legacy default 3 fps.
    set_test_capture(fake_capture());
    socket
        .send(WsMessage::Text(
            json!({"type":"start","display":display,"maxWidth":320,"quality":60})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    let meta = next_meta(&mut socket).await;
    assert_eq!(meta["codec"], "jpeg");
    assert_eq!(meta["fps"], 3);
    assert_eq!(meta["display"], display);

    // 2. h264 requested while the host cannot create a session: honest fallback.
    let previous = screen::force_availability_for_tests(Some(false));
    set_test_capture(fake_capture());
    socket
        .send(WsMessage::Text(
            json!({"type":"start","display":display,"fps":30,"maxWidth":320,
                   "quality":60,"codec":"h264","bitrate":4000})
            .to_string()
            .into(),
        ))
        .await
        .unwrap();
    let meta = next_meta(&mut socket).await;
    assert_eq!(meta["codec"], "jpeg", "unusable h264 must report jpeg");
    // A requested 30 fps clamps to the JPEG ceiling, not the JPEG default.
    assert_eq!(meta["fps"], 5);
    screen::force_availability_for_tests(previous);

    // 3. h264 start round trip: meta says h264 and a decodable IDR arrives.
    if screen::available_codecs().contains(&"h264") {
        // Parallel VideoToolbox sessions would slow the encoder benchmark test.
        let _encoder_guard = screen::lock_test_encoder();
        set_test_capture(fake_capture());
        socket
            .send(WsMessage::Text(
                json!({"type":"start","display":display,"fps":30,"maxWidth":320,
                       "quality":60,"codec":"h264","bitrate":4000})
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
        let meta = next_meta(&mut socket).await;
        assert_eq!(meta["codec"], "h264");
        assert_eq!(meta["fps"], 30);
        let packet = next_binary(&mut socket).await;
        let frame = screen::decode_h264_frame(&packet).expect("frozen wire framing");
        assert!(frame.header.key, "the first h264 frame must be an IDR");
        assert_eq!(frame.header.ps.len(), 2);
        assert_eq!((frame.header.w, frame.header.h), (320, 240));
    } else {
        eprintln!("skipping h264 round trip: VideoToolbox unavailable on this host");
    }
    server.abort();
}
