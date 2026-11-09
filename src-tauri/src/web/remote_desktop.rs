//! Authenticated remote desktop transport. Pixels and input text never enter logs.
use std::{
    net::{IpAddr, SocketAddr},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};

use axum::{
    body::Body,
    extract::{
        ws::{CloseFrame, Message, WebSocket},
        ConnectInfo, Extension, Query, WebSocketUpgrade,
    },
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot};

use super::{
    auth::{RemoteAccessAuthority, RemoteAuthError, RemotePrincipal},
    upgraded_connections::{UpgradedConnectionKind, UpgradedConnectionTicket},
};
use crate::{
    remote::desktop::DesktopService,
    screen::{
        self, CaptureOptions, ImageFormat, InputController, Modifiers, MouseButton, StreamConfig,
        StreamEvent, StreamReceiver, StreamSession,
    },
};

const VIDEO_QUEUE: usize = 4;
static NEXT_VIEWER: AtomicU64 = AtomicU64::new(1);
type DesktopResult<T> = Result<T, &'static str>;

fn error(code: &'static str) -> Response {
    let status = match code {
        "UNAUTHORIZED" => StatusCode::UNAUTHORIZED,
        "DISPLAY_NOT_FOUND" => StatusCode::NOT_FOUND,
        "SCREEN_UNSUPPORTED" => StatusCode::NOT_IMPLEMENTED,
        "CAPTURE_FAILED" | "INPUT_FAILED" => StatusCode::INTERNAL_SERVER_ERROR,
        "INVALID_REQUEST" => StatusCode::BAD_REQUEST,
        _ => StatusCode::FORBIDDEN,
    };
    (status, Json(json!({"error": code, "code": code}))).into_response()
}

fn gate(supported: bool, view: bool, control: bool, input: bool) -> DesktopResult<()> {
    if !supported {
        return Err("SCREEN_UNSUPPORTED");
    }
    if !view {
        return Err("DESKTOP_DISABLED");
    }
    if input && !control {
        return Err("CONTROL_DISABLED");
    }
    Ok(())
}

fn authorize(
    authority: &RemoteAccessAuthority,
    principal: &RemotePrincipal,
    service: &DesktopService,
    input: bool,
) -> DesktopResult<()> {
    authority
        .require_scope(principal, "screen.view")
        .map_err(|e| e.code())?;
    if input {
        authority
            .require_scope(principal, "screen.control")
            .map_err(|e| e.code())?;
    }
    gate(
        screen::supported(),
        service.view_enabled(),
        service.control_enabled(),
        input,
    )
}

pub async fn info(
    Extension(authority): Extension<Arc<RemoteAccessAuthority>>,
    Extension(principal): Extension<RemotePrincipal>,
    service: Option<Extension<Arc<DesktopService>>>,
) -> Response {
    let service = service.map_or_else(DesktopService::global, |Extension(s)| s);
    if let Err(e) = authority.require_scope(&principal, "screen.view") {
        return error(e.code());
    }
    // Unsupported info is deliberately discoverable; no native display API is called on Linux.
    if screen::supported() && !service.view_enabled() {
        return error("DESKTOP_DISABLED");
    }
    let mut value = serde_json::to_value(service.settings()).unwrap_or_else(|_| json!({}));
    let displays = if screen::supported() {
        match screen::list_displays_async().await {
            Ok(v) => v,
            Err(screen::ScreenError::ScreenPermissionRequired) => vec![],
            Err(e) => return error(e.code()),
        }
    } else {
        vec![]
    };
    value["displays"] = json!(displays);
    Json(value).into_response()
}

#[derive(Deserialize)]
pub struct ScreenshotQuery {
    display: String,
    format: Option<String>,
    quality: Option<u8>,
}

pub async fn screenshot(
    Extension(authority): Extension<Arc<RemoteAccessAuthority>>,
    Extension(principal): Extension<RemotePrincipal>,
    Query(query): Query<ScreenshotQuery>,
) -> Response {
    let service = DesktopService::global();
    if let Err(code) = authorize(&authority, &principal, &service, false) {
        return error(code);
    }
    let format = match query.format.as_deref().unwrap_or("jpeg") {
        "jpeg" => ImageFormat::Jpeg {
            quality: query.quality.unwrap_or(80).clamp(1, 100),
        },
        "png" => ImageFormat::Png,
        _ => return error("INVALID_REQUEST"),
    };
    let id = NEXT_VIEWER.fetch_add(1, Ordering::Relaxed);
    let mut stop = service.subscribe_stop();
    service.viewer_started(id, query.display.clone());
    let result = tokio::select! {
        biased;
        _ = stop.recv() => Err(screen::ScreenError::CaptureFailed("desktop stopped".into())),
        result = screen::capture_async(query.display.clone(), CaptureOptions { format, ..Default::default() }) => result,
    };
    service.viewer_stopped(id);
    // Do not deliver a capture that raced desktop consent or device revocation.
    let result = authorize(&authority, &principal, &service, false)
        .and_then(|_| result.map_err(|e| e.code()));
    let code = result.as_ref().map(|_| "OK").unwrap_or_else(|e| *e);
    log::info!(
        "desktop_screenshot device_id={} display={} stable_code={}",
        principal.device_id(),
        query.display,
        code
    );
    match result {
        Ok(image) => Response::builder()
            .header("content-type", image.mime)
            .header("cache-control", "no-store")
            .body(Body::from(image.bytes))
            .unwrap()
            .into_response(),
        Err(code) => error(code),
    }
}

// Defense in depth: a paired device cannot grant itself consent, even via loopback.
fn require_desktop_operator(
    principal: Option<Extension<RemotePrincipal>>,
    ingress: Option<Extension<super::auth::IngressProvenance>>,
    peer: SocketAddr,
) -> DesktopResult<()> {
    let Some(Extension(principal)) = principal else {
        return Err("UNAUTHORIZED");
    };
    if !peer.ip().is_loopback()
        || !principal.is_legacy()
        || !matches!(
            ingress,
            Some(Extension(super::auth::IngressProvenance::LocalOperator))
        )
    {
        return Err("FORBIDDEN");
    }
    Ok(())
}
// These routes are classified LocalOperator, never remotely callable even with a paired token.
pub async fn get_settings(
    principal: Option<Extension<RemotePrincipal>>,
    ingress: Option<Extension<super::auth::IngressProvenance>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
) -> Response {
    if let Err(code) = require_desktop_operator(principal, ingress, peer) {
        return error(code);
    }
    Json(DesktopService::global().settings()).into_response()
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SettingsBody {
    view_enabled: bool,
    control_enabled: bool,
}
pub async fn set_settings(
    principal: Option<Extension<RemotePrincipal>>,
    ingress: Option<Extension<super::auth::IngressProvenance>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Json(body): Json<SettingsBody>,
) -> Response {
    if let Err(code) = require_desktop_operator(principal, ingress, peer) {
        return error(code);
    }
    match DesktopService::global().set_settings(body.view_enabled, body.control_enabled) {
        Ok(settings) => Json(settings).into_response(),
        Err(_) => error("SETTINGS_FAILED"),
    }
}
pub async fn emergency_stop(
    principal: Option<Extension<RemotePrincipal>>,
    ingress: Option<Extension<super::auth::IngressProvenance>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
) -> Response {
    if let Err(code) = require_desktop_operator(principal, ingress, peer) {
        return error(code);
    }
    match DesktopService::global().emergency_stop() {
        Ok(()) => Json(json!({"ok":true})).into_response(),
        Err(_) => error("SETTINGS_FAILED"),
    }
}

pub async fn stream(
    Extension(authority): Extension<Arc<RemoteAccessAuthority>>,
    registry: Option<Extension<Arc<super::upgraded_connections::UpgradedConnectionRegistry>>>,
    service: Option<Extension<Arc<DesktopService>>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    let principal = match upgrade_principal(&authority, peer.ip(), &headers) {
        Ok(v) => v,
        Err(e) => return error(e.code()),
    };
    let registry = registry.map_or_else(
        super::upgraded_connections::UpgradedConnectionRegistry::global,
        |Extension(r)| r,
    );
    let service = service.map_or_else(DesktopService::global, |Extension(s)| s);
    ws.max_message_size(64 * 1024)
        .on_upgrade(move |socket| async move {
            let ticket = match &principal {
                Some(p) => registry.admit_device(UpgradedConnectionKind::Desktop, p.device_id()),
                None => registry.admit(UpgradedConnectionKind::Desktop),
            };
            if let Some(ticket) = ticket {
                run(socket, authority, principal, peer.ip(), ticket, service).await;
            }
        })
        .into_response()
}
fn upgrade_principal(
    authority: &RemoteAccessAuthority,
    peer: IpAddr,
    headers: &HeaderMap,
) -> Result<Option<RemotePrincipal>, RemoteAuthError> {
    authority.verify_origin(headers.get(axum::http::header::ORIGIN))?;
    let Some(header) = headers.get(axum::http::header::AUTHORIZATION) else {
        return Ok(None);
    };
    let token = header
        .to_str()
        .ok()
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .ok_or(RemoteAuthError::InvalidCredential)?;
    let principal = authority.verify_bearer_for_peer(token, peer)?;
    authority.require_scope(&principal, "screen.view")?;
    Ok(Some(principal))
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum ClientMessage {
    Authenticate {
        #[serde(default)]
        id: Option<String>,
        payload: AuthPayload,
    },
    Start {
        display: String,
        fps: Option<u32>,
        #[serde(rename = "maxWidth")]
        max_width: Option<u32>,
        quality: Option<u8>,
    },
    SwitchDisplay {
        display: String,
    },
    Keyframe,
    Stop,
    Pointer {
        display: String,
        u: f64,
        v: f64,
    },
    Button {
        button: MouseButton,
        down: bool,
    },
    Click {
        display: String,
        button: MouseButton,
        count: u8,
    },
    Scroll {
        display: String,
        dx: i32,
        dy: i32,
    },
    Key {
        code: String,
        down: bool,
        #[serde(default)]
        modifiers: Modifiers,
    },
    Text {
        text: String,
    },
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthPayload {
    token: String,
}
impl ClientMessage {
    fn input(&self) -> bool {
        matches!(
            self,
            Self::Pointer { .. }
                | Self::Button { .. }
                | Self::Click { .. }
                | Self::Scroll { .. }
                | Self::Key { .. }
                | Self::Text { .. }
        )
    }
}

struct LiveStream {
    session: StreamSession,
    receiver: StreamReceiver,
    display: String,
    fps: u32,
}
impl Drop for LiveStream {
    fn drop(&mut self) {
        self.session.stop();
    }
}

fn send_json(tx: &mpsc::Sender<Message>, value: Value) -> bool {
    tx.try_send(Message::Text(value.to_string().into())).is_ok()
}
fn send_error(tx: &mpsc::Sender<Message>, code: &'static str) -> bool {
    send_json(tx, json!({"type":"error","code":code}))
}
// A transport drop invalidates delta ancestry; recovery is mandatory and never awaits the writer.
fn send_frame(tx: &mpsc::Sender<Message>, bytes: Vec<u8>, recover: impl FnOnce()) -> bool {
    match tx.try_send(Message::Binary(bytes.into())) {
        Ok(()) => true,
        Err(mpsc::error::TrySendError::Full(_)) => {
            recover();
            true
        }
        Err(mpsc::error::TrySendError::Closed(_)) => false,
    }
}

async fn run(
    socket: WebSocket,
    authority: Arc<RemoteAccessAuthority>,
    mut principal: Option<RemotePrincipal>,
    peer: IpAddr,
    ticket: UpgradedConnectionTicket,
    service: Arc<DesktopService>,
) {
    let id = NEXT_VIEWER.fetch_add(1, Ordering::Relaxed);
    let cancel = ticket.cancel_token();
    if let Some(p) = &principal {
        ticket.bind_device(p.device_id());
    }
    let mut stops = service.subscribe_stop();
    let (mut sink, mut source) = socket.split();
    let (tx, mut rx) = mpsc::channel(VIDEO_QUEUE);
    let (close_tx, mut close_rx) = oneshot::channel::<(u16, &'static str)>();
    let writer = tokio::spawn(async move {
        let close = loop {
            tokio::select! {
                biased;
                reason = &mut close_rx => break reason.unwrap_or((1000,"closed")),
                msg = rx.recv() => match msg {
                    Some(msg) => {
                        tokio::select! {
                            biased;
                            reason = &mut close_rx => break reason.unwrap_or((1000,"closed")),
                            sent = tokio::time::timeout(Duration::from_secs(10), sink.send(msg)) => {
                                if !matches!(sent, Ok(Ok(()))) { break (1001,"slow consumer") }
                            }
                        }
                    }
                    None => break (1000,"closed"),
                }
            }
        };
        let _ = tokio::time::timeout(
            Duration::from_secs(1),
            sink.send(Message::Close(Some(CloseFrame {
                code: close.0,
                reason: close.1.into(),
            }))),
        )
        .await;
    });
    let mut stream: Option<LiveStream> = None;
    let mut input: Option<InputController> = None;
    let mut control_active = false;
    let mut last_control = service.control_enabled();
    let mut pump = tokio::time::interval(Duration::from_millis(15));
    pump.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut heartbeat = tokio::time::interval(Duration::from_secs(20));
    heartbeat.tick().await;
    let auth_deadline = tokio::time::sleep(Duration::from_secs(10));
    tokio::pin!(auth_deadline);
    let mut code = (1000, "closed");
    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => { code = (1008,"device access revoked"); break; }
            _ = stops.recv() => { code = (1008,"desktop stopped"); break; }
            _ = &mut auth_deadline, if principal.is_none() => { code = (1008,"authentication required"); break; }
            _ = heartbeat.tick() => {
                if tx.try_send(Message::Ping(vec![].into())).is_err() { break; }
            }
            _ = pump.tick() => {
                if writer.is_finished() { break; }
                if let Some(p) = &principal {
                    if authority.require_scope(p,"screen.view").is_err() { code=(1008,"device access revoked"); break; }
                    let enabled = service.control_enabled();
                    if !enabled { input = None; control_active = false; }
                    if enabled != last_control {
                        last_control = enabled;
                        if !send_json(&tx,json!({"type":"control_state","enabled":enabled,"active":input.is_some()})) {break}
                    }
                }
                if let Some(live) = &stream {
                    if !service.view_enabled() { code=(1008,"desktop disabled"); break; }
                    match live.receiver.try_recv() {
                        Ok(StreamEvent::Frame(packet)) => { if !send_frame(&tx,packet,|| live.session.request_keyframe()) { break; } }
                        Ok(StreamEvent::Error(e)) => { send_error(&tx,e.code()); stop_stream(&mut stream,&service,id,&principal,"CAPTURE_FAILED"); }
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => { stop_stream(&mut stream,&service,id,&principal,"OK"); }
                        Err(std::sync::mpsc::TryRecvError::Empty) => {}
                    }
                }
            }
            message = source.next() => {
                let text = match message {
                    Some(Ok(Message::Text(text))) => text,
                    Some(Ok(Message::Ping(bytes))) => { if tx.try_send(Message::Pong(bytes)).is_err(){break} continue },
                    Some(Ok(Message::Pong(_))) => continue,
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                    _ => { if !send_error(&tx,"INVALID_REQUEST") {break} continue },
                };
                let msg = match serde_json::from_str::<ClientMessage>(&text) { Ok(msg) => msg, Err(_) => { if !send_error(&tx,"INVALID_REQUEST") {break} continue } };
                if let ClientMessage::Authenticate { id: request_id, payload } = msg {
                    if principal.is_none() {
                        match authority.verify_bearer_for_peer(&payload.token,peer).and_then(|p| {authority.require_scope(&p,"screen.view")?;Ok(p)}) {
                            Ok(p) => { ticket.bind_device(p.device_id()); principal=Some(p); }
                            Err(e) => { send_error(&tx,e.code()); code=(1008,"authentication rejected"); break; }
                        }
                    }
                    if !send_json(&tx,json!({"id":request_id,"type":"response","success":true,"payload":{}})) {break}
                    continue;
                }
                let Some(p) = &principal else { send_error(&tx,"UNAUTHORIZED"); code=(1008,"authentication required"); break };
                if let Err(rejected) = authorize(&authority,p,&service,msg.input()) {
                    if msg.input() { log::warn!("desktop_input_rejected device_id={} display={} stable_code={}",p.device_id(),stream.as_ref().map(|s|s.display.as_str()).unwrap_or("none"),rejected); }
                    if !send_error(&tx,rejected) {break} continue;
                }
                let result: DesktopResult<()> = match msg {
                    ClientMessage::Start {display,fps,max_width,quality} => {
                        let fps = fps.unwrap_or(3).clamp(1,5);
                        match screen::list_displays_async().await {
                            Err(e) => Err(e.code()),
                            Ok(displays) if !displays.iter().any(|d|d.id==display) => Err("DISPLAY_NOT_FOUND"),
                            Ok(displays) => {
                                stop_stream(&mut stream,&service,id,&principal,"OK"); input=None; control_active=false;
                                match StreamSession::start(StreamConfig {display:display.clone(),fps,max_width:max_width.unwrap_or(1920),quality:quality.unwrap_or(80),queue_capacity:2}) {
                                    Err(e) => Err(e.code()),
                                    Ok((session,receiver)) => {
                                        service.viewer_started(id,display.clone());
                                        log::info!("desktop_stream_start device_id={} display={} stable_code=OK",p.device_id(),display);
                                        stream=Some(LiveStream{session,receiver,display:display.clone(),fps});
                                        if !send_json(&tx,json!({"type":"meta","displays":displays,"display":display,"fps":fps})) {break}
                                        if !send_json(&tx,json!({"type":"control_state","enabled":service.control_enabled(),"active":false})) {break}
                                        Ok(())
                                    }
                                }
                            }
                        }
                    }
                    ClientMessage::SwitchDisplay{display} => {
                        match (&mut stream,screen::list_displays_async().await) {
                            (_,Err(e))=>Err(e.code()),
                            (Some(live),Ok(displays)) if displays.iter().any(|d|d.id==display) => {
                                match live.session.switch_display(display.clone()) {
                                    Err(e) => Err(e.code()),
                                    Ok(()) => {
                                        live.display=display.clone();
                                        service.set_viewer_display(id,display.clone());
                                        if !send_json(&tx,json!({"type":"meta","displays":displays,"display":display,"fps":live.fps})){break}
                                        Ok(())
                                    }
                                }
                            }
                            _=>Err("DISPLAY_NOT_FOUND"),
                        }
                    }
                    ClientMessage::Keyframe => { if let Some(live)=&stream {live.session.request_keyframe();Ok(())} else {Err("INVALID_REQUEST")} }
                    ClientMessage::Stop => {
                        stop_stream(&mut stream,&service,id,&principal,"OK");input=None;control_active=false;
                        if !send_json(&tx,json!({"type":"control_state","enabled":service.control_enabled(),"active":false})){break}
                        Ok(())
                    }
                    msg => {
                        let result=inject(msg,&stream,&mut input);
                        if result.is_ok() && !control_active {
                            control_active=true;
                            service.controller_active(id);
                            if !send_json(&tx,json!({"type":"control_state","enabled":true,"active":true})){break}
                        }
                        if let Err(rejected)=&result {
                            log::warn!("desktop_input_rejected device_id={} display={} stable_code={}",p.device_id(),stream.as_ref().map(|s|s.display.as_str()).unwrap_or("none"),rejected);
                        }
                        result
                    }
                };
                if let Err(e)=result {if !send_error(&tx,e){break}}
            }
        }
    }
    stop_stream(
        &mut stream,
        &service,
        id,
        &principal,
        if code.0 == 1008 { "FORBIDDEN" } else { "OK" },
    );
    drop(input);
    let _ = close_tx.send(code);
    drop(tx);
    let _ = writer.await;
}

fn stop_stream(
    stream: &mut Option<LiveStream>,
    service: &DesktopService,
    id: u64,
    principal: &Option<RemotePrincipal>,
    code: &str,
) {
    if let Some(live) = stream.take() {
        log::info!(
            "desktop_stream_stop device_id={} display={} stable_code={}",
            principal
                .as_ref()
                .map(|p| p.device_id())
                .unwrap_or("legacy"),
            live.display,
            code
        );
        drop(live);
    }
    service.viewer_stopped(id);
}
fn inject(
    msg: ClientMessage,
    stream: &Option<LiveStream>,
    input: &mut Option<InputController>,
) -> DesktopResult<()> {
    let display = &stream.as_ref().ok_or("INVALID_REQUEST")?.display;
    if input.is_none() {
        *input = Some(InputController::new().map_err(|e| e.code())?)
    }
    let controller = input.as_ref().ok_or("INPUT_PERMISSION_REQUIRED")?;
    let result = match msg {
        ClientMessage::Pointer { display: d, u, v } if &d == display => {
            controller.pointer_move(display, u, v)
        }
        ClientMessage::Button { button, down } => controller.button(display, button, down),
        ClientMessage::Click {
            display: d,
            button,
            count,
        } if &d == display => controller.click(display, &button, count),
        ClientMessage::Scroll { display: d, dx, dy } if &d == display => {
            controller.scroll(display, dx, dy)
        }
        ClientMessage::Key {
            code,
            down,
            modifiers,
        } => controller.key(display, &code, down, modifiers),
        ClientMessage::Text { text } => controller.text(display, &text),
        _ => return Err("INVALID_REQUEST"),
    };
    result.map_err(|e| e.code())
}

#[cfg(test)]
#[path = "remote_desktop_tests.rs"]
mod transport_tests;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn gate_defaults_and_unsupported() {
        assert_eq!(gate(true, false, false, false), Err("DESKTOP_DISABLED"));
        assert_eq!(gate(true, true, false, true), Err("CONTROL_DISABLED"));
        assert_eq!(gate(false, true, false, false), Err("SCREEN_UNSUPPORTED"));
        assert!(gate(true, true, false, false).is_ok());
    }
    #[test]
    fn ws_messages_parse_and_reject_ambiguous_payloads() {
        for text in [
            r#"{"type":"start","display":"1","maxWidth":1024,"fps":3}"#,
            r#"{"type":"switch_display","display":"2"}"#,
            r#"{"type":"keyframe"}"#,
            r#"{"type":"stop"}"#,
            r#"{"type":"pointer","display":"1","u":0.3,"v":0.4}"#,
            r#"{"type":"button","button":"left","down":true}"#,
            r#"{"type":"click","display":"1","button":"left","count":2}"#,
            r#"{"type":"scroll","display":"1","dx":0,"dy":20}"#,
            r#"{"type":"key","code":"KeyA","down":true,"modifiers":{"shift":true}}"#,
            r#"{"type":"text","text":"hello"}"#,
            r#"{"id":"1","type":"authenticate","payload":{"token":"secret"}}"#,
        ] {
            assert!(
                serde_json::from_str::<ClientMessage>(text).is_ok(),
                "{text}"
            );
        }
        assert!(serde_json::from_str::<ClientMessage>(
            r#"{"type":"start","display":"1","token":"bad"}"#
        )
        .is_err());
        assert!(serde_json::from_str::<ClientMessage>(
            r#"{"type":"pointer","display":"1","u":null,"v":0}"#
        )
        .is_err());
    }
    #[tokio::test]
    async fn full_video_queue_drops_without_waiting_and_requests_keyframe() {
        let (tx, mut rx) = mpsc::channel(1);
        tx.try_send(Message::Binary(vec![1].into())).unwrap();
        let recovered = std::sync::atomic::AtomicBool::new(false);
        assert!(send_frame(&tx, vec![2], || recovered.store(true, Ordering::SeqCst)));
        assert!(recovered.load(Ordering::SeqCst));
        assert!(matches!(rx.try_recv(),Ok(Message::Binary(bytes)) if bytes.as_ref()==[1]));
        assert!(rx.try_recv().is_err());
        drop(rx);
        assert!(!send_frame(&tx, vec![3], || panic!(
            "closed transport is not pressure"
        )));
    }
}
