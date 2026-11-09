//! Loopback-only receiver. Ownership comes from the host registry, never payload.
use super::{contract::HookIdentityDto, install, state, HookKind};
use axum::{
    extract::{ConnectInfo, DefaultBodyLimit, Path, State},
    http::{HeaderMap, StatusCode},
    routing::post,
    Router,
};
use std::{
    collections::HashMap,
    future::Future,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::{Path as FsPath, PathBuf},
    pin::Pin,
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant},
};
use tokio::sync::Semaphore;

#[derive(Clone, Debug)]
pub struct HookOwner {
    pub project_id: String,
    pub owner_session_id: String,
    pub cwd: String,
    /// Only host launch/process evidence may select among multiple variants.
    pub variant: Option<String>,
    pub root_pid: Option<u32>,
}
pub(crate) type OwnerResolver = Arc<
    dyn Fn(String, u64) -> Pin<Box<dyn Future<Output = Option<HookOwner>> + Send>> + Send + Sync,
>;
static OWNER: OnceLock<Mutex<Option<OwnerResolver>>> = OnceLock::new();
static OWNED_TERMINALS: OnceLock<Mutex<std::collections::HashSet<String>>> = OnceLock::new();
/// Called only by trusted host spawn paths after receiving an actual PTY ID.
pub fn register_owned_terminal(terminal: &str) {
    OWNED_TERMINALS
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .insert(terminal.to_owned());
}
pub fn owns_terminal(terminal: &str) -> bool {
    OWNED_TERMINALS
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .contains(terminal)
}
/// A very fast agent start may race the spawn RPC reply. Await only that bounded
/// registration window, never accept a terminal merely because list() shows it.
pub async fn await_owned_terminal(terminal: &str) -> bool {
    for _ in 0..8 {
        if owns_terminal(terminal) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    owns_terminal(terminal)
}
type VariantResolver =
    Arc<dyn Fn(String, u64) -> Pin<Box<dyn Future<Output = Option<String>> + Send>> + Send + Sync>;
static VARIANT_RESOLVER: OnceLock<Mutex<Option<VariantResolver>>> = OnceLock::new();
static VARIANTS: OnceLock<Mutex<HashMap<String, (u64, String, String)>>> = OnceLock::new();
pub fn register_variant_resolver(resolver: VariantResolver) {
    *VARIANT_RESOLVER
        .get_or_init(Default::default)
        .lock()
        .unwrap() = Some(resolver);
}
pub fn register_owner_resolver(resolver: OwnerResolver) {
    *OWNER.get_or_init(Default::default).lock().unwrap() = Some(resolver);
}

#[derive(Clone)]
struct ReceiverState {
    secret: Arc<String>,
    instance: Arc<String>,
    permits: Arc<Semaphore>,
    home: PathBuf,
    #[cfg(test)]
    owner_resolver: Option<OwnerResolver>,
}
static STARTED: OnceLock<Result<(), String>> = OnceLock::new();
static ENVIRONMENT: OnceLock<HashMap<String, String>> = OnceLock::new();
static ENDPOINT_FILE: OnceLock<PathBuf> = OnceLock::new();
pub fn receiver_environment() -> HashMap<String, String> {
    ENVIRONMENT.get().cloned().unwrap_or_default()
}

#[derive(Default)]
struct Rejects {
    counts: HashMap<&'static str, u64>,
    last_summary: Option<Instant>,
}
static REJECTS: OnceLock<Mutex<Rejects>> = OnceLock::new();
fn reject(reason: &'static str, code: StatusCode) -> StatusCode {
    let mut rejects = REJECTS.get_or_init(Default::default).lock().unwrap();
    *rejects.counts.entry(reason).or_default() += 1;
    rejects.last_summary.get_or_insert_with(Instant::now);
    code
}
fn summarize_rejects() {
    let mut rejects = REJECTS.get_or_init(Default::default).lock().unwrap();
    if !rejects.counts.is_empty()
        && rejects
            .last_summary
            .is_some_and(|t| t.elapsed() >= Duration::from_secs(60))
    {
        tracing::warn!(target: "agent_hooks", reasons = ?rejects.counts, "receiver_reject_summary");
        rejects.counts.clear();
        rejects.last_summary = Some(Instant::now());
    }
}
fn header<'a>(headers: &'a HeaderMap, key: &str) -> Option<&'a str> {
    headers.get(key)?.to_str().ok()
}
fn authorized(peer: SocketAddr, headers: &HeaderMap, secret: &str) -> bool {
    if peer.ip() != IpAddr::V4(Ipv4Addr::LOCALHOST) {
        return false;
    }
    let Some(value) = header(headers, "authorization").and_then(|s| s.strip_prefix("Bearer "))
    else {
        return false;
    };
    // Constant work for equal-sized secrets. No forwarded headers are consulted.
    value.len() == secret.len()
        && value
            .bytes()
            .zip(secret.bytes())
            .fold(0u8, |a, (b, c)| a | (b ^ c))
            == 0
}
fn qin_envelope(headers: &HeaderMap, payload: &mut serde_json::Value) -> Result<(), ()> {
    let fixed = |name: &str, max: usize| {
        header(headers, name)
            .filter(|s| !s.is_empty() && s.len() <= max && !s.chars().any(char::is_control))
            .map(str::to_owned)
            .ok_or(())
    };
    let object = payload.as_object_mut().ok_or(())?;
    if !object.contains_key("event") {
        object.insert(
            "event".into(),
            serde_json::Value::String(fixed("x-se-hook-event", 128)?),
        );
    }
    if !object.contains_key("session_id") {
        object.insert(
            "session_id".into(),
            serde_json::Value::String(fixed("x-se-hook-session-id", 512)?),
        );
    }
    Ok(())
}
fn router(state: ReceiverState) -> Router {
    Router::new()
        .route("/hooks/{adapter}", post(receive))
        .layer(DefaultBodyLimit::max(256 * 1024))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            admission,
        ))
        .with_state(state)
}
async fn admission(
    State(receiver): State<ReceiverState>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let peer = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|peer| peer.0);
    if !peer.is_some_and(|peer| authorized(peer, request.headers(), &receiver.secret)) {
        return reject("authorization", StatusCode::FORBIDDEN).into_response();
    }
    let Ok(_permit) = receiver.permits.clone().try_acquire_owned() else {
        return reject("concurrency", StatusCode::TOO_MANY_REQUESTS).into_response();
    };
    match tokio::time::timeout(Duration::from_secs(5), next.run(request)).await {
        Ok(response) => response,
        Err(_) => reject("deadline", StatusCode::REQUEST_TIMEOUT).into_response(),
    }
}

async fn receive(
    State(receiver): State<ReceiverState>,
    Path(adapter): Path<String>,
    headers: HeaderMap,
    body: String,
) -> StatusCode {
    if header(&headers, "x-se-hook-instance") != Some(receiver.instance.as_str()) {
        return reject("instance", StatusCode::FORBIDDEN);
    }
    let Some(terminal) =
        header(&headers, "x-se-hook-pty-id").filter(|s| !s.is_empty() && s.len() <= 128)
    else {
        return reject("terminal", StatusCode::FORBIDDEN);
    };
    let Some(generation) =
        header(&headers, "x-se-hook-generation").and_then(|s| s.parse::<u64>().ok())
    else {
        return reject("generation", StatusCode::FORBIDDEN);
    };
    let Some(install_id) =
        header(&headers, "x-se-hook-install-id").filter(|s| uuid::Uuid::parse_str(s).is_ok())
    else {
        return reject("install", StatusCode::FORBIDDEN);
    };
    if super::contract::AdapterId::try_from(adapter.clone()).is_err() {
        return reject("adapter", StatusCode::BAD_REQUEST);
    }
    let resolver = OWNER.get_or_init(Default::default).lock().unwrap().clone();
    #[cfg(test)]
    let resolver = receiver.owner_resolver.clone().or(resolver);
    let Some(resolver) = resolver else {
        return reject("registry_unavailable", StatusCode::FORBIDDEN);
    };
    let owner = tokio::time::timeout(
        Duration::from_millis(100),
        resolver(terminal.to_owned(), generation),
    )
    .await;
    let Ok(Some(mut owner)) = owner else {
        return reject("ownership", StatusCode::FORBIDDEN);
    };
    let Ok(mut raw) = serde_json::from_str::<serde_json::Value>(&body) else {
        return reject("payload", StatusCode::BAD_REQUEST);
    };
    if adapter == "qin-code" {
        if qin_envelope(&headers, &mut raw).is_err() {
            return reject("qin_identity_headers", StatusCode::BAD_REQUEST);
        }
    }
    let home = &receiver.home;
    let normalized = match install::normalize_installed(&home, &adapter, install_id, &raw) {
        Ok(event) => event,
        Err(_) => return reject("snapshot_or_payload", StatusCode::BAD_REQUEST),
    };
    let kind = normalized.kind;
    if adapter == "qin-code" {
        if kind == HookKind::SessionStart {
            if owner.variant.is_none() {
                let resolver = VARIANT_RESOLVER
                    .get_or_init(Default::default)
                    .lock()
                    .unwrap()
                    .clone();
                if let Some(resolver) = resolver {
                    owner.variant = tokio::time::timeout(
                        Duration::from_secs(2),
                        resolver(terminal.to_owned(), generation),
                    )
                    .await
                    .ok()
                    .flatten();
                }
            }
            let mut cache = VARIANTS.get_or_init(Default::default).lock().unwrap();
            cache.remove(terminal);
            if let Some(variant) = owner
                .variant
                .as_ref()
                .filter(|v| matches!(v.as_str(), "default" | "self-dev"))
            {
                cache.insert(
                    terminal.to_owned(),
                    (generation, normalized.session_id.clone(), variant.clone()),
                );
            }
        } else if owner.variant.is_none() {
            owner.variant = VARIANTS
                .get_or_init(Default::default)
                .lock()
                .unwrap()
                .get(terminal)
                .filter(|(g, s, _)| *g == generation && *s == normalized.session_id)
                .map(|(_, _, variant)| variant.clone());
        }
    }
    let previous_identity = state::latest(terminal);
    let next = state::upsert_with(
        state::StateInput {
            terminal_id: terminal.to_owned(),
            generation,
            adapter_install_id: install_id.to_owned(),
            project_id: owner.project_id.clone(),
            identity: HookIdentityDto {
                adapter_id: adapter.try_into().unwrap(),
                agent_session_id: normalized.session_id,
                transcript_path: normalized.transcript_path,
                cwd: Some(owner.cwd.clone()),
                source: super::contract::HookIdentitySource::Hook,
                descriptor_id: None,
                resume_argv: None,
            },
            kind,
        },
        |next, previous| persist_identity_edge(&home, &owner, kind, next, previous),
    );
    if let Some(value) = next.as_ref().filter(|_| kind != HookKind::SessionEnd) {
        let identity_changed = previous_identity.as_ref().is_none_or(|old| {
            old.generation != value.generation
                || old.adapter_install_id != value.adapter_install_id
                || old.identity != value.identity
        });
        if kind == HookKind::SessionStart || identity_changed {
            if let (Some(root_pid), Some(descriptor_id)) =
                (owner.root_pid, value.identity.descriptor_id.clone())
            {
                let evidence_home = home.clone();
                let _ = tokio::task::spawn_blocking(move || {
                    crate::cli_session::restore_service::capture_process_evidence(
                        &evidence_home,
                        &descriptor_id,
                        root_pid,
                    )
                })
                .await;
            }
        }
    }
    if kind == HookKind::SessionEnd {
        VARIANTS
            .get_or_init(Default::default)
            .lock()
            .unwrap()
            .remove(terminal);
    }
    // Only lifecycle edges log. Tool events are memory-only no-ops in the reducer.
    if next.is_some() && matches!(kind, HookKind::SessionStart | HookKind::SessionEnd) {
        tracing::info!(target: "agent_hooks", edge = ?kind, "session_lifecycle");
    }
    StatusCode::NO_CONTENT
}
/// Invoked inside the reducer's pre-publication critical section, exclusively
/// on an observable state/identity edge. C services here must not reenter state.
fn persist_identity_edge(
    home: &FsPath,
    owner: &HookOwner,
    kind: HookKind,
    next: &mut super::contract::HookStateDto,
    previous: Option<&super::contract::HookStateDto>,
) {
    use super::contract::{HookErrorCode, RestoreState};
    use crate::cli_session::{identity_store, restore_service};
    if kind == HookKind::SessionEnd {
        if let Some(descriptor) = next.identity.descriptor_id.as_deref() {
            if identity_store::mark_ended(home, descriptor, next.generation).is_err() {
                reject("identity_end", StatusCode::SERVICE_UNAVAILABLE);
            }
        }
        return;
    }
    let identity_changed = previous.is_none_or(|old| {
        old.generation != next.generation
            || old.adapter_install_id != next.adapter_install_id
            || old.identity.adapter_id != next.identity.adapter_id
            || old.agent_session_id != next.agent_session_id
            || old.identity.transcript_path != next.identity.transcript_path
            || old.identity.cwd != next.identity.cwd
            || old.identity.source != next.identity.source
    });
    if !identity_changed && kind != HookKind::SessionStart {
        return;
    }
    if next.identity.adapter_id.as_str() == "qin-code"
        && !matches!(owner.variant.as_deref(), Some("default" | "self-dev"))
    {
        // Never silently choose the default resume template when host proof failed.
        if let Some(id) = next.identity.descriptor_id.as_deref() {
            if let Ok(_guard) = identity_store::lock(home, id) {
                if let Ok(mut stored) = identity_store::load(home, id) {
                    if stored.terminal_id == next.terminal_id
                        && stored.generation == next.generation
                    {
                        stored.restore_state = RestoreState::Stale;
                        let _ = identity_store::save(home, &stored);
                    }
                }
            }
        }
        next.restore_state = RestoreState::Stale;
        next.identity.descriptor_id = None;
        reject("variant_unproven", StatusCode::CONFLICT);
        return;
    }
    if kind == HookKind::SessionStart {
        match restore_service::confirm_verified_start(home, &owner.owner_session_id, next) {
            Ok(Some(identity)) => {
                next.identity = identity;
                next.restore_state = RestoreState::Confirmed;
                return;
            }
            Ok(None) => {}
            Err(error) => {
                next.restore_state = if error.code == HookErrorCode::IdentityStale {
                    RestoreState::Stale
                } else {
                    RestoreState::Unconfirmed
                };
                reject("identity_confirmation", StatusCode::CONFLICT);
                return;
            }
        }
    }
    let snapshot_ref = match install::snapshot_ref_for(
        home,
        next.identity.adapter_id.as_str(),
        &next.adapter_install_id,
    ) {
        Ok(path) => path.to_string_lossy().into_owned(),
        Err(_) => {
            next.restore_state = RestoreState::SnapshotMissing;
            reject("identity_snapshot", StatusCode::CONFLICT);
            return;
        }
    };
    if next.identity.adapter_id.as_str() == "qin-code" && owner.variant.is_none() {
        next.restore_state = RestoreState::Unconfirmed;
    }
    match identity_store::upsert_verified_identity(
        home,
        &owner.owner_session_id,
        next,
        &snapshot_ref,
        owner.variant.as_deref().unwrap_or("default"),
    ) {
        Ok(identity) => {
            next.identity = identity;
        }
        Err(error) => {
            next.restore_state = if error.code == HookErrorCode::IdentityStale {
                RestoreState::Stale
            } else {
                RestoreState::Unconfirmed
            };
            reject("identity_write", StatusCode::CONFLICT);
        }
    }
}

fn private_dir(path: &FsPath) -> Result<(), String> {
    if let Ok(meta) = std::fs::symlink_metadata(path) {
        if meta.file_type().is_symlink() || !meta.is_dir() {
            return Err("unsafe receiver directory".into());
        }
    } else {
        if let Some(parent) = path.parent() {
            if !parent.exists() {
                private_dir(parent)?;
            }
        }
        std::fs::create_dir(path).map_err(|e| e.to_string())?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}
fn write_endpoint(
    home: &FsPath,
    instance: &str,
    endpoint: &str,
    secret: &str,
) -> Result<PathBuf, String> {
    private_dir(&home.join(".se-manager"))?;
    let root = home.join(".se-manager/agent-hooks");
    private_dir(&root)?;
    let instances = root.join("instances");
    private_dir(&instances)?;
    let own = instances.join(instance);
    private_dir(&own)?;
    let path = own.join("endpoint.env");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    use std::io::Write;
    let mut file = options.open(&path).map_err(|e| e.to_string())?;
    write!(file, "SE_AGENT_HOOK_INSTANCE={instance}\nSE_AGENT_HOOK_ENDPOINT={endpoint}\nSE_AGENT_HOOK_TOKEN={secret}\n").map_err(|e| e.to_string())?;
    Ok(path)
}
/// Cleanup is deliberately nonrecursive and only removes our process's endpoint.
fn remove_own_endpoint(path: &FsPath) {
    let _ = std::fs::remove_file(path);
    if let Some(dir) = path.parent() {
        let _ = std::fs::remove_dir(dir);
    }
}
pub fn shutdown() {
    if let Some(path) = ENDPOINT_FILE.get() {
        remove_own_endpoint(path);
    }
}
extern "C" fn cleanup_at_exit() {
    shutdown();
}
fn bind_loopback() -> std::io::Result<std::net::TcpListener> {
    std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
}
pub fn start() -> Result<(), String> {
    STARTED
        .get_or_init(|| {
            let listener = bind_loopback().map_err(|e| e.to_string())?;
            listener.set_nonblocking(true).map_err(|e| e.to_string())?;
            let endpoint = format!(
                "http://127.0.0.1:{}",
                listener.local_addr().map_err(|e| e.to_string())?.port()
            );
            let secret = format!(
                "{}{}",
                uuid::Uuid::new_v4().simple(),
                uuid::Uuid::new_v4().simple()
            );
            let instance = state::instance_id().to_owned();
            let home = std::env::var_os("HOME")
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
                .ok_or("home unavailable")?;
            let path = write_endpoint(&home, &instance, &endpoint, &secret)?;
            let receiver = ReceiverState {
                secret: Arc::new(secret.clone()),
                instance: Arc::new(instance.clone()),
                permits: Arc::new(Semaphore::new(32)),
                home: home.clone(),
                #[cfg(test)]
                owner_resolver: None,
            };
            let _ = ENVIRONMENT.set(HashMap::from([
                ("SE_AGENT_HOOK_INSTANCE".into(), instance),
                ("SE_AGENT_HOOK_ENDPOINT".into(), endpoint),
                ("SE_AGENT_HOOK_TOKEN".into(), secret),
            ]));
            se_pty::manager::configure_agent_hook_environment(receiver_environment());
            let _ = ENDPOINT_FILE.set(path);
            unsafe {
                libc::atexit(cleanup_at_exit);
            }
            std::thread::Builder::new()
                .name("agent-hook-receiver".into())
                .spawn(move || {
                    let runtime = match tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                    {
                        Ok(r) => r,
                        Err(_) => {
                            shutdown();
                            return;
                        }
                    };
                    runtime.block_on(async move {
                        tokio::spawn(crate::cli_session::identity_gc::run(receiver.home.clone()));
                        let listener = match tokio::net::TcpListener::from_std(listener) {
                            Ok(l) => l,
                            Err(_) => {
                                shutdown();
                                return;
                            }
                        };
                        tokio::spawn(async {
                            let mut tick = tokio::time::interval(Duration::from_secs(60));
                            loop {
                                tick.tick().await;
                                summarize_rejects();
                                state::collect();
                            }
                        });
                        tracing::info!(target: "agent_hooks", "receiver_start");
                        if axum::serve(
                            listener,
                            router(receiver).into_make_service_with_connect_info::<SocketAddr>(),
                        )
                        .await
                        .is_err()
                        {
                            tracing::error!(target: "agent_hooks", "receiver_stopped");
                        }
                        shutdown();
                    });
                })
                .map_err(|e| {
                    shutdown();
                    e.to_string()
                })?;
            Ok(())
        })
        .clone()
}
#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;
    #[tokio::test]
    async fn native_generation_zero_environment_accepts_session_start() {
        use super::super::contract::{
            AdapterId, HookAction, HookConfirmRequest, HookPreviewRequest,
        };
        let home = tempfile::tempdir().unwrap();
        let adapter = AdapterId::try_from("claude-code".to_string()).unwrap();
        let preview = install::preview_v3(
            home.path(),
            HookPreviewRequest {
                adapter_id: adapter.clone(),
                action: HookAction::Install,
                event_options: Default::default(),
            },
        )
        .unwrap();
        let installed = install::confirm_v3(
            home.path(),
            HookConfirmRequest {
                adapter_id: adapter,
                preview_id: preview.preview_id,
                fingerprint: preview.fingerprint,
            },
            Some(HookAction::Install),
        )
        .unwrap();
        let mut receiver = receiver();
        receiver.home = home.path().into();
        let env_file = home.path().join("native-env");
        let manager = crate::web::test_pty_manager();
        receiver.owner_resolver = Some(crate::commands::hook_owner_resolver(
            crate::core::TerminalServiceHandle::in_process(manager.clone()),
        ));
        let spawned = manager
            .spawn(
                se_pty::SpawnOptions {
                    program: Some("/bin/sh".into()),
                    cwd: Some(home.path().to_string_lossy().into_owned()),
                    args: Some(vec![
                        "-c".into(),
                        "env > \"$1\"; sleep 30".into(),
                        "sh".into(),
                        env_file.to_string_lossy().into_owned(),
                    ]),
                    env: Some(HashMap::from([
                        (
                            "SE_AGENT_HOOK_ENDPOINT".into(),
                            "http://127.0.0.1:42001".into(),
                        ),
                        ("SE_AGENT_HOOK_TOKEN".into(), receiver.secret.to_string()),
                        (
                            "SE_AGENT_HOOK_INSTANCE".into(),
                            receiver.instance.to_string(),
                        ),
                    ])),
                    project_id: Some("native-project".into()),
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();
        let terminal_id = spawned.info.id.clone();
        register_owned_terminal(&terminal_id);
        let mut injected = HashMap::new();
        for _ in 0..200 {
            if let Ok(text) = std::fs::read_to_string(&env_file) {
                injected = text
                    .lines()
                    .filter_map(|line| line.split_once('='))
                    .map(|(k, v)| (k.to_owned(), v.to_owned()))
                    .collect();
                if injected.contains_key("SE_AGENT_HOOK_GENERATION") {
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(
            injected.get("SE_AGENT_HOOK_GENERATION").map(String::as_str),
            Some("0")
        );
        assert_eq!(injected["SE_AGENT_HOOK_PTY_ID"], terminal_id);
        assert!(
            receiver.owner_resolver.as_ref().unwrap()(terminal_id.clone(), 1)
                .await
                .is_none()
        );
        let session_id = uuid::Uuid::new_v4().to_string();
        let payload = serde_json::json!({
            "hook_event_name": "SessionStart", "session_id": session_id,
            "cwd": home.path().to_str().unwrap(),
        });
        let response = router(receiver.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/hooks/claude-code")
                    .header("authorization", format!("Bearer {}", receiver.secret))
                    .header("x-se-hook-instance", &injected["SE_AGENT_HOOK_INSTANCE"])
                    .header("x-se-hook-pty-id", &injected["SE_AGENT_HOOK_PTY_ID"])
                    .header(
                        "x-se-hook-generation",
                        &injected["SE_AGENT_HOOK_GENERATION"],
                    )
                    .header("x-se-hook-install-id", installed.status.install_id.unwrap())
                    .extension(ConnectInfo(
                        "127.0.0.1:42001".parse::<SocketAddr>().unwrap(),
                    ))
                    .body(Body::from(payload.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        manager.terminate(&terminal_id).await.unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        let state = state::latest(&terminal_id).expect("native SessionStart must upsert state");
        assert_eq!(state.generation, 0);
        assert_eq!(state.identity.agent_session_id, session_id);
    }

    #[tokio::test(start_paused = true)]
    async fn admission_bounds_body_receipt_before_polling_and_times_out_stalled_streams() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        fn stream_request(token: &str, polls: Arc<AtomicUsize>) -> Request<Body> {
            let stream = futures_util::stream::poll_fn(move |_| {
                polls.fetch_add(1, Ordering::SeqCst);
                std::task::Poll::<Option<Result<axum::body::Bytes, std::io::Error>>>::Pending
            });
            Request::builder()
                .method("POST")
                .uri("/hooks/claude-code")
                .header("authorization", format!("Bearer {token}"))
                .extension(ConnectInfo(
                    "127.0.0.1:42001".parse::<SocketAddr>().unwrap(),
                ))
                .body(Body::from_stream(stream))
                .unwrap()
        }
        let mut state = receiver();
        state.permits = Arc::new(Semaphore::new(1));
        let first_polls = Arc::new(AtomicUsize::new(0));
        let first = tokio::spawn(
            router(state.clone()).oneshot(stream_request(&state.secret, first_polls.clone())),
        );
        while first_polls.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
        let rejected_polls = Arc::new(AtomicUsize::new(0));
        let response = router(state.clone())
            .oneshot(stream_request("unauthorized", rejected_polls.clone()))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let response = router(state.clone())
            .oneshot(stream_request(&state.secret, rejected_polls.clone()))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(rejected_polls.load(Ordering::SeqCst), 0);
        tokio::time::advance(Duration::from_secs(5)).await;
        assert_eq!(
            first.await.unwrap().unwrap().status(),
            StatusCode::REQUEST_TIMEOUT
        );
        assert_eq!(state.permits.available_permits(), 1);
    }

    fn receiver() -> ReceiverState {
        ReceiverState {
            secret: Arc::new("a".repeat(64)),
            instance: Arc::new(uuid::Uuid::new_v4().to_string()),
            permits: Arc::new(Semaphore::new(32)),
            home: PathBuf::new(),
            owner_resolver: None,
        }
    }
    async fn request(state: ReceiverState, peer: SocketAddr, instance: &str) -> StatusCode {
        let mut request = Request::post("/hooks/pi")
            .header("authorization", format!("Bearer {}", "a".repeat(64)))
            .header("x-se-hook-instance", instance)
            .header(
                "x-se-hook-install-id",
                "74b74834-5205-4c1e-a731-35ab983994b5",
            )
            .header("x-se-hook-pty-id", "terminal-1")
            .header("x-se-hook-generation", "1")
            .body(Body::from("{}"))
            .unwrap();
        request.extensions_mut().insert(ConnectInfo(peer));
        router(state).oneshot(request).await.unwrap().status()
    }
    #[tokio::test]
    async fn cross_instance_and_forged_forwarded_peer_rejected_without_changes() {
        let state = receiver();
        let original = state.instance.to_string();
        assert_eq!(
            request(
                state.clone(),
                "127.0.0.1:9".parse().unwrap(),
                &uuid::Uuid::new_v4().to_string()
            )
            .await,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            request(state, "192.0.2.1:9".parse().unwrap(), &original).await,
            StatusCode::FORBIDDEN
        );
    }
    #[test]
    fn qin_header_envelope_only_fills_missing_fields_and_preserves_raw_json_values() {
        let mut headers = HeaderMap::new();
        headers.insert("x-se-hook-event", "SessionStart".parse().unwrap());
        headers.insert("x-se-hook-session-id", "session-header".parse().unwrap());
        let mut payload = serde_json::json!({"text":"quote\" newline\n unicode你好"});
        qin_envelope(&headers, &mut payload).unwrap();
        assert_eq!(payload["event"], "SessionStart");
        assert_eq!(payload["session_id"], "session-header");
        assert_eq!(payload["text"], "quote\" newline\n unicode你好");
        let mut supplied = serde_json::json!({"event":"TurnDone", "session_id":"from-payload"});
        qin_envelope(&headers, &mut supplied).unwrap();
        assert_eq!(supplied["event"], "TurnDone");
        assert_eq!(supplied["session_id"], "from-payload");
        assert!(qin_envelope(&HeaderMap::new(), &mut serde_json::json!({})).is_err());
        assert!(qin_envelope(&headers, &mut serde_json::json!([])).is_err());
        headers.insert("x-se-hook-event", "x".repeat(129).parse().unwrap());
        assert!(qin_envelope(&headers, &mut serde_json::json!({})).is_err());
    }

    #[test]
    fn qin_without_host_variant_proof_is_durably_stale_not_default_resumable() {
        use super::super::contract::*;
        use crate::cli_session::identity_store;
        let home = tempfile::tempdir().unwrap();
        let mut next = HookStateDto {
            instance_id: "host".into(),
            terminal_id: "terminal".into(),
            generation: 1,
            adapter_install_id: "install".into(),
            agent_session_id: "session".into(),
            project_id: "project".into(),
            state: DerivedState::Idle,
            ready: false,
            identity: HookIdentityDto {
                adapter_id: "qin-code".to_owned().try_into().unwrap(),
                agent_session_id: "session".into(),
                transcript_path: None,
                cwd: None,
                source: HookIdentitySource::Hook,
                descriptor_id: None,
                resume_argv: None,
            },
            restore_state: RestoreState::Confirmed,
            revision: 1,
        };
        next.identity = identity_store::upsert_verified_identity(
            home.path(),
            "owner",
            &next,
            "snapshot",
            "self-dev",
        )
        .unwrap();
        let descriptor_id = next.identity.descriptor_id.clone().unwrap();
        let owner = HookOwner {
            project_id: "project".into(),
            owner_session_id: "owner".into(),
            cwd: "/work".into(),
            variant: None,
            root_pid: None,
        };
        persist_identity_edge(home.path(), &owner, HookKind::SessionStart, &mut next, None);
        assert_eq!(next.restore_state, RestoreState::Stale);
        assert!(next.identity.descriptor_id.is_none());
        let stored = identity_store::load(home.path(), &descriptor_id).unwrap();
        assert_eq!(stored.restore_state, RestoreState::Stale);
        assert_eq!(stored.variant, "self-dev");
    }

    #[test]
    fn endpoint_is_private_per_instance_and_never_overwrites_global() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join(".se-manager/agent-hooks");
        private_dir(&root).unwrap();
        std::fs::write(root.join("endpoint.env"), "legacy").unwrap();
        let one = write_endpoint(
            home.path(),
            &uuid::Uuid::new_v4().to_string(),
            "http://127.0.0.1:1",
            "one",
        )
        .unwrap();
        let two = write_endpoint(
            home.path(),
            &uuid::Uuid::new_v4().to_string(),
            "http://127.0.0.1:2",
            "two",
        )
        .unwrap();
        assert_ne!(one, two);
        assert!(one.exists() && two.exists());
        assert_eq!(
            std::fs::read_to_string(root.join("endpoint.env")).unwrap(),
            "legacy"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(one).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
    #[test]
    fn listener_is_ephemeral_ipv4_loopback() {
        let listener = bind_loopback().unwrap();
        assert_eq!(
            listener.local_addr().unwrap().ip(),
            IpAddr::V4(Ipv4Addr::LOCALHOST)
        );
        assert_ne!(listener.local_addr().unwrap().port(), 0);
    }
}
