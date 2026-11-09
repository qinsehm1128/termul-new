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
    fixture_with_input(control, None).await
}

async fn injected_stream(
    ws: WebSocketUpgrade,
    Extension(authority): Extension<Arc<RemoteAccessAuthority>>,
    Extension(service): Extension<Arc<DesktopService>>,
    Extension(registry): Extension<Arc<UpgradedConnectionRegistry>>,
    Extension(input): Extension<Arc<std::sync::Mutex<Option<InputController>>>>,
) -> Response {
    let controller = input.lock().unwrap().take();
    ws.on_upgrade(move |socket| async move {
        let ticket = registry.admit(UpgradedConnectionKind::Desktop).unwrap();
        run_with_input(
            socket,
            authority,
            None,
            "127.0.0.1".parse().unwrap(),
            ticket,
            service,
            controller,
        )
        .await;
    })
    .into_response()
}

async fn fixture_with_input(control: bool, input: Option<InputController>) -> Fixture {
    let injected = input.is_some();
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
        .route(
            "/remote/desktop/stream",
            if injected {
                get(injected_stream)
            } else {
                get(stream)
            },
        )
        .layer(Extension(Arc::new(std::sync::Mutex::new(input))))
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
async fn next_json(socket: &mut WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>) -> Value {
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(2), socket.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        match frame {
            WsMessage::Text(text) => return serde_json::from_str(&text).unwrap(),
            WsMessage::Ping(_) | WsMessage::Pong(_) => continue,
            other => panic!("expected JSON text, got {other:?}"),
        }
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
        if scope_missing {
            assert_eq!(
                next_json(&mut f.socket).await,
                json!({"type":"control_state","enabled":false,"active":false})
            );
            assert_eq!(
                next_json(&mut f.socket).await,
                json!({"type":"error","code":code,"capability":capability})
            );
            f.socket
                .send(WsMessage::Ping(vec![9].into()))
                .await
                .unwrap();
            let frame = tokio::time::timeout(Duration::from_secs(2), f.socket.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            assert!(matches!(frame, WsMessage::Pong(data) if data.as_ref()==[9]));
            assert!(f.authority.verify_bearer(&f.token).is_ok());
        } else {
            assert_shutdown(&mut f, code, capability, 1000).await;
        }
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

#[tokio::test]
async fn control_revocation_flushes_queue_reports_forbidden_and_keeps_view_socket_open() {
    let (controller, probe) = InputController::for_tests(None);
    assert_eq!(probe.queued(), 1);
    assert!(probe.emit());
    let mut f = fixture_with_input(true, Some(controller)).await;
    f.authority
        .set_device_scopes(&f.device_id, vec!["screen.view".into()])
        .unwrap();
    for expected in [
        json!({"type":"control_state","enabled":false,"active":false}),
        json!({"type":"error","code":"FORBIDDEN","capability":"screen.control"}),
    ] {
        assert_eq!(next_json(&mut f.socket).await, expected);
    }
    assert_eq!(probe.queued(), 0);
    assert!(!probe.emit(), "no native emit after controller revoke");
    f.socket
        .send(WsMessage::Ping(vec![7].into()))
        .await
        .unwrap();
    let frame = tokio::time::timeout(Duration::from_secs(2), f.socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(matches!(frame, WsMessage::Pong(data) if data.as_ref()==[7]));
    assert!(f.authority.verify_bearer(&f.token).is_ok());
    // A subsequent forbidden input is also non-terminal while view remains valid.
    f.socket
        .send(WsMessage::Text(
            json!({"type":"text","text":"denied"}).to_string().into(),
        ))
        .await
        .unwrap();
    for expected in [
        json!({"type":"control_state","enabled":false,"active":false}),
        json!({"type":"error","code":"FORBIDDEN","capability":"screen.control"}),
    ] {
        assert_eq!(next_json(&mut f.socket).await, expected);
    }
}

#[test]
fn successful_display_switch_drop_barrier_flushes_old_display_requests() {
    // The successful SwitchDisplay arm invokes this same synchronous barrier
    // before updating the display or sending new metadata; failed switches don't.
    let service = DesktopService::for_tests(true, true);
    let (controller, probe) = InputController::for_tests(None);
    let mut input = Some(controller);
    let mut active = true;
    drop_controller(&mut input, &mut active, &service, 42);
    assert!(input.is_none());
    assert!(!active);
    assert_eq!(probe.queued(), 0);
    assert!(
        !probe.emit(),
        "dequeued old-display work must also be invalid"
    );
}

#[tokio::test]
async fn pump_recheck_preserves_authorized_controller_then_flushes_control_only_revoke() {
    let f = fixture(true).await;
    let principal = f.authority.verify_bearer(&f.token).unwrap();
    let (controller, probe) = InputController::for_tests(None);
    let mut input = Some(controller);
    let mut active = true;
    let (tx, mut rx) = mpsc::channel(4);
    assert!(recheck_controller(
        &tx,
        &f.authority,
        &principal,
        &mut input,
        &mut active,
        &f.service,
        42
    ));
    assert!(input.is_some());
    assert_eq!(probe.queued(), 1);
    assert!(rx.try_recv().is_err());
    f.authority
        .set_device_scopes(&f.device_id, vec!["screen.view".into()])
        .unwrap();
    assert!(recheck_controller(
        &tx,
        &f.authority,
        &principal,
        &mut input,
        &mut active,
        &f.service,
        42
    ));
    assert!(input.is_none());
    assert!(!active);
    assert_eq!(probe.queued(), 0);
    assert!(!probe.emit());
    assert!(f.authority.require_scope(&principal, "screen.view").is_ok());
    for expected in [
        json!({"type":"control_state","enabled":false,"active":false}),
        json!({"type":"error","code":"FORBIDDEN","capability":"screen.control"}),
    ] {
        let frame = rx.try_recv().unwrap();
        assert!(
            matches!(frame, Message::Text(text) if serde_json::from_str::<Value>(&text).unwrap() == expected)
        );
    }
    // No repeated revoke errors on later pump ticks after the controller is gone.
    assert!(recheck_controller(
        &tx,
        &f.authority,
        &principal,
        &mut input,
        &mut active,
        &f.service,
        42
    ));
    assert!(rx.try_recv().is_err());
}
