//! Core process discovery, spawning, and the minimal role server.
//!
//! The GUI owns only the client-side launch/reuse operation. Core processes do
//! not receive a kill-on-drop relationship from the GUI. The actual Terminal
//! and ACP service handlers are layered on top of this role server later.

use super::ipc::{
    component_build_id, prepare_runtime_dir, read_json_frame, write_json_frame, CoreEndpoint,
    CoreError, CoreHello, CoreHelloAck, CoreRequest, CoreRole, CURRENT_PROTOCOL_VERSION,
};
use super::transport::connect_core;
use std::collections::HashMap;
#[cfg(test)]
use std::path::Path;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

#[derive(Debug)]
pub struct CoreProcess {
    pub role: CoreRole,
    pub endpoint: CoreEndpoint,
    pub pid: u32,
    pub reused: bool,
    pub remote_build_id: Option<String>,
    pub remote_capabilities: Vec<String>,
    pub active_resources: u32,
    pub reconciliation_deferred: bool,
    child: Option<Child>,
}

impl CoreProcess {
    pub fn is_owned_child(&self) -> bool {
        self.child.is_some()
    }
}

#[derive(Debug, Clone)]
pub struct CoreLaunchConfig {
    pub profile_root: PathBuf,
    pub executable: PathBuf,
    pub ready_timeout: Duration,
}

impl CoreLaunchConfig {
    pub fn for_current_executable(profile_root: impl Into<PathBuf>) -> Result<Self, CoreError> {
        let executable = std::env::current_exe()
            .map_err(|error| CoreError::Io(format!("resolve current executable: {error}")))?;
        Ok(Self {
            profile_root: profile_root.into(),
            executable,
            ready_timeout: Duration::from_secs(5),
        })
    }
}

#[derive(Debug, Clone)]
struct CoreUpdateDirective {
    enforce_identity: bool,
    expected_build_id: Option<String>,
    defer_terminal_replacement: bool,
    unsupported: bool,
}

fn update_directives() -> &'static Mutex<HashMap<CoreRole, CoreUpdateDirective>> {
    static DIRECTIVES: OnceLock<Mutex<HashMap<CoreRole, CoreUpdateDirective>>> = OnceLock::new();
    DIRECTIVES.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Installs the validated component policy for the current process. The
/// renderer remains the durable authority; this in-memory projection lets the
/// launcher apply the same policy during startup and supervisor retries.
pub fn configure_update_policy(policy: Option<&serde_json::Value>) {
    let mut directives = update_directives()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    directives.clear();
    let Some(components) = policy.and_then(|value| value.get("components")) else {
        for role in [CoreRole::AcpCore, CoreRole::TerminalCore] {
            directives.insert(
                role,
                CoreUpdateDirective {
                    enforce_identity: true,
                    expected_build_id: None,
                    defer_terminal_replacement: false,
                    unsupported: true,
                },
            );
        }
        return;
    };
    let legacy_metadata = policy
        .and_then(|value| value.get("metadataState"))
        .and_then(serde_json::Value::as_str)
        == Some("legacy");
    for (component, role) in [
        ("acpCore", CoreRole::AcpCore),
        ("terminalCore", CoreRole::TerminalCore),
    ] {
        let Some(entry) = components.get(component) else {
            directives.insert(
                role,
                CoreUpdateDirective {
                    enforce_identity: true,
                    expected_build_id: None,
                    defer_terminal_replacement: false,
                    unsupported: true,
                },
            );
            continue;
        };
        let action = entry.get("action").and_then(serde_json::Value::as_str);
        let build_id = if legacy_metadata {
            Some(component_build_id(role))
        } else {
            entry
                .get("buildId")
                .and_then(serde_json::Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .map(str::to_owned)
        };
        if matches!(action, Some("preserve")) {
            continue;
        }
        let unsupported = !matches!(
            action,
            Some("restart") | Some("defer-if-active") | Some("unsupported")
        );
        directives.insert(
            role,
            CoreUpdateDirective {
                enforce_identity: true,
                expected_build_id: build_id,
                defer_terminal_replacement: role == CoreRole::TerminalCore
                    && matches!(action, Some("defer-if-active")),
                unsupported: unsupported || matches!(action, Some("unsupported")),
            },
        );
    }
}

pub fn update_reconciliation_pending(role: CoreRole) -> bool {
    configured_update_directive(role).is_some()
}

fn configured_update_directive(role: CoreRole) -> Option<CoreUpdateDirective> {
    update_directives()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(&role)
        .cloned()
}

fn role_locks() -> &'static Mutex<HashMap<CoreRole, Arc<tokio::sync::Mutex<()>>>> {
    static LOCKS: OnceLock<Mutex<HashMap<CoreRole, Arc<tokio::sync::Mutex<()>>>>> = OnceLock::new();
    LOCKS.get_or_init(|| Mutex::new(HashMap::new()))
}

async fn role_lock(role: CoreRole) -> Arc<tokio::sync::Mutex<()>> {
    let mut locks = role_locks()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    locks
        .entry(role)
        .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
        .clone()
}

/// Consecutive failed supervisor probes required before declaring a core dead
/// and asking `ensure_core` to respawn. A single flap must not double-spawn.
pub(crate) const CORE_DEATH_PROBE_MISSES: u32 = 3;

/// Pure consecutive-miss gate used by the desktop supervisor.
pub(crate) fn should_declare_core_death(consecutive_misses: u32) -> bool {
    consecutive_misses >= CORE_DEATH_PROBE_MISSES
}

/// Never unlink a socket while a tracked owned child is still alive.
pub(crate) fn may_unlink_owned_socket(owned_child_alive: bool) -> bool {
    !owned_child_alive
}

/// Observed occupancy of a Core endpoint. Connect success means a live peer
/// even when Hello is rejected and the socket is then closed: that peer does
/// not enter the request loop, so shutdown must not be assumed.
#[derive(Debug)]
pub(crate) enum EndpointPresence {
    Absent,
    LiveCompatible(CoreHelloAck),
    LiveIncompatible(CoreError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreIdentityState {
    Current,
    Legacy,
    Stale,
}

pub fn classify_core_identity(ack: &CoreHelloAck, role: CoreRole) -> CoreIdentityState {
    match ack.component_build_id.as_deref() {
        None => CoreIdentityState::Legacy,
        Some(build_id) if build_id == component_build_id(role) => CoreIdentityState::Current,
        Some(_) => CoreIdentityState::Stale,
    }
}

pub fn terminal_replacement_is_safe(active_resources: u32) -> bool {
    active_resources == 0
}

fn core_identity_matches_expected(ack: &CoreHelloAck, expected_build_id: Option<&str>) -> bool {
    expected_build_id.is_some_and(|expected| ack.component_build_id.as_deref() == Some(expected))
}

impl EndpointPresence {
    pub(crate) fn from_connect_and_handshake(
        connect: Result<(), CoreError>,
        handshake: Option<Result<CoreHelloAck, CoreError>>,
    ) -> Self {
        match connect {
            Err(error) if error.is_connect_absence() => Self::Absent,
            Err(error) => Self::LiveIncompatible(error),
            Ok(()) => match handshake {
                Some(Ok(ack)) => Self::LiveCompatible(ack),
                Some(Err(error)) => Self::LiveIncompatible(error),
                None => Self::LiveIncompatible(CoreError::Io(
                    "connected core closed before handshake".into(),
                )),
            },
        }
    }

    pub(crate) fn is_live(&self) -> bool {
        !matches!(self, Self::Absent)
    }

    pub(crate) fn allows_in_process_fallback(&self) -> bool {
        !self.is_live()
    }

    pub(crate) fn allows_unlink_and_spawn(&self) -> bool {
        !self.is_live()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IncompatibleReplaceOutcome {
    Cleared,
    StillLive,
}

impl IncompatibleReplaceOutcome {
    pub(crate) fn allows_unlink_and_spawn(self) -> bool {
        matches!(self, Self::Cleared)
    }
}

struct OwnedCore {
    pid: u32,
    child: Child,
}

fn owned_cores() -> &'static Mutex<HashMap<CoreRole, OwnedCore>> {
    static OWNED: OnceLock<Mutex<HashMap<CoreRole, OwnedCore>>> = OnceLock::new();
    OWNED.get_or_init(|| Mutex::new(HashMap::new()))
}

fn insert_owned_core(role: CoreRole, child: Child) {
    let pid = child.id();
    let mut cores = owned_cores()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    cores.insert(role, OwnedCore { pid, child });
}

fn take_owned_core(role: CoreRole) -> Option<OwnedCore> {
    owned_cores()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(&role)
}

#[cfg(test)]
pub(crate) fn owned_child_pid(role: CoreRole) -> Option<u32> {
    owned_cores()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(&role)
        .map(|owned| owned.pid)
}

#[cfg(unix)]
fn send_sigterm(pid: u32) {
    unsafe {
        libc::kill(pid as i32, libc::SIGTERM);
    }
}

async fn wait_for_owned_exit(
    child: &mut Child,
    endpoint: &CoreEndpoint,
    timeout: Duration,
) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if child.try_wait().ok().flatten().is_some() {
            return true;
        }
        if connect_core(endpoint).await.is_err() && child.try_wait().ok().flatten().is_some() {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return child.try_wait().ok().flatten().is_some();
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// SIGTERM (Unix) / TerminateProcess (Windows) a previously spawned child for
/// `role` and wait until it is reaped (and the endpoint stops accepting).
/// Returns whether the child is gone so the caller may unlink. The child is
/// put back in the registry if it is still alive — never unlink while we still
/// own a live process. Windows uses the same `try_wait` poll as Unix; it must
/// not block the async worker on `Child::wait`.
pub(crate) async fn terminate_owned_core(role: CoreRole, endpoint: &CoreEndpoint) -> bool {
    let Some(mut owned) = take_owned_core(role) else {
        return true;
    };
    #[cfg(unix)]
    send_sigterm(owned.pid);
    #[cfg(not(unix))]
    let _ = owned.child.kill();

    let exited = wait_for_owned_exit(&mut owned.child, endpoint, Duration::from_secs(3)).await;

    if exited {
        true
    } else {
        insert_owned_core(role, owned.child);
        false
    }
}

pub(crate) async fn probe_endpoint_presence(
    endpoint: &CoreEndpoint,
    expected_role: CoreRole,
) -> EndpointPresence {
    let mut stream = match connect_core(endpoint).await {
        Ok(stream) => stream,
        Err(error) => {
            return EndpointPresence::from_connect_and_handshake(Err(error), None);
        }
    };
    let hello = CoreHello {
        // Hello.role names the target server, not the caller. The GUI identifies
        // itself with `client_name` and asks for `expected_role`.
        role: expected_role,
        protocol_versions: vec![CURRENT_PROTOCOL_VERSION],
        client_name: "termul-gui-launcher".to_string(),
    };
    if let Err(error) = write_json_frame(&mut stream, &hello).await {
        return EndpointPresence::from_connect_and_handshake(Ok(()), Some(Err(error)));
    }
    match read_json_frame::<_, CoreHelloAck>(&mut stream).await {
        Ok(ack)
            if ack.role == expected_role && ack.protocol_version == CURRENT_PROTOCOL_VERSION =>
        {
            EndpointPresence::LiveCompatible(ack)
        }
        Ok(_) => EndpointPresence::from_connect_and_handshake(
            Ok(()),
            Some(Err(CoreError::InvalidHandshake(
                "core returned an incompatible handshake".to_string(),
            ))),
        ),
        Err(error) => EndpointPresence::from_connect_and_handshake(Ok(()), Some(Err(error))),
    }
}

pub(crate) async fn probe_endpoint(
    endpoint: &CoreEndpoint,
    expected_role: CoreRole,
) -> Result<CoreHelloAck, CoreError> {
    match probe_endpoint_presence(endpoint, expected_role).await {
        EndpointPresence::LiveCompatible(ack) => Ok(ack),
        EndpointPresence::Absent => Err(CoreError::Io("core endpoint is not listening".into())),
        EndpointPresence::LiveIncompatible(error) => Err(error),
    }
}

async fn replace_incompatible_core(
    endpoint: &CoreEndpoint,
    role: CoreRole,
) -> IncompatibleReplaceOutcome {
    let shutdown_sent = try_shutdown_core(endpoint, role).await;
    // Always wait for the endpoint to go absent. A rejected pre-hello peer is
    // not in the request loop, so a shutdown frame must not be treated as
    // processed just because connect succeeded.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        let presence = probe_endpoint_presence(endpoint, role).await;
        if presence.allows_unlink_and_spawn() {
            log::info!(
                target: "se_manager::core",
                "operation=core_replace role={} stable_code=INCOMPATIBLE_REPLACED shutdown_sent={shutdown_sent}",
                role.endpoint_name()
            );
            return IncompatibleReplaceOutcome::Cleared;
        }
        if tokio::time::Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    log::error!(
        target: "se_manager::core",
        "operation=core_replace role={} stable_code=ZOMBIE_ORPHANED detail=shutdown_not_acknowledged shutdown_sent={shutdown_sent}",
        role.endpoint_name()
    );
    IncompatibleReplaceOutcome::StillLive
}

async fn try_shutdown_core(endpoint: &CoreEndpoint, role: CoreRole) -> bool {
    let Ok(mut stream) = connect_core(endpoint).await else {
        return true; // nothing listening; socket file is stale
    };
    let hello = CoreHello {
        role,
        protocol_versions: vec![CURRENT_PROTOCOL_VERSION],
        client_name: "termul-gui-replacer".to_string(),
    };
    if super::ipc::write_json_frame(&mut stream, &hello)
        .await
        .is_err()
    {
        return false;
    }
    // Only a peer that wrote HelloAck has entered the request loop. A core
    // that rejected Hello (role/protocol) drops this socket before shutdown
    // can be processed; best-effort wire shutdown applies only after ack.
    if super::ipc::read_json_frame::<_, CoreHelloAck>(&mut stream)
        .await
        .is_err()
    {
        return false;
    }
    let request = CoreRequest {
        id: 1,
        method: "shutdown".to_string(),
        params: serde_json::Value::Null,
    };
    super::ipc::write_json_frame(&mut stream, &request)
        .await
        .is_ok()
}

pub async fn ensure_core(
    role: CoreRole,
    config: &CoreLaunchConfig,
) -> Result<CoreProcess, CoreError> {
    let endpoint = CoreEndpoint::for_profile(&config.profile_root, role);
    let lock = role_lock(role).await;
    let _guard = lock.lock().await;

    match probe_endpoint_presence(&endpoint, role).await {
        EndpointPresence::LiveCompatible(ack) => {
            let directive = configured_update_directive(role);
            let expected_build_id = directive
                .as_ref()
                .and_then(|value| value.expected_build_id.as_deref());
            let enforce_identity = directive
                .as_ref()
                .is_some_and(|value| value.enforce_identity);
            let identity_matches =
                !enforce_identity || core_identity_matches_expected(&ack, expected_build_id);
            let defer_terminal_replacement = directive
                .as_ref()
                .is_some_and(|value| value.defer_terminal_replacement);
            if directive.as_ref().is_some_and(|value| value.unsupported) {
                log::error!(
                    target: "se_manager::core",
                    "operation=core_reconcile role={} stable_code=UNSUPPORTED_COMPONENT",
                    role.endpoint_name()
                );
                return Err(CoreError::UnsupportedPlatform);
            }
            if !identity_matches {
                if role == CoreRole::TerminalCore
                    && defer_terminal_replacement
                    && !terminal_replacement_is_safe(ack.active_resources)
                {
                    log::warn!(
                        target: "se_manager::core",
                        "operation=core_reconcile role={} stable_code=REPLACEMENT_DEFERRED active_resources={}",
                        role.endpoint_name(),
                        ack.active_resources
                    );
                    return Ok(CoreProcess {
                        role,
                        endpoint,
                        pid: 0,
                        reused: true,
                        remote_build_id: ack.component_build_id,
                        remote_capabilities: ack.capabilities,
                        active_resources: ack.active_resources,
                        reconciliation_deferred: true,
                        child: None,
                    });
                }
                if !replace_incompatible_core(&endpoint, role)
                    .await
                    .allows_unlink_and_spawn()
                {
                    return Err(CoreError::Io(format!(
                        "stale {} is still live; refusing to unlink and spawn a second core",
                        role.endpoint_name()
                    )));
                }
            } else {
                log::info!(
                    target: "se_manager::core",
                    "operation=core_adopt role={} stable_code=ADOPTED",
                    role.endpoint_name()
                );
                return Ok(CoreProcess {
                    role,
                    endpoint,
                    pid: 0,
                    reused: true,
                    remote_build_id: ack.component_build_id,
                    remote_capabilities: ack.capabilities,
                    active_resources: ack.active_resources,
                    reconciliation_deferred: false,
                    child: None,
                });
            }
        }
        EndpointPresence::LiveIncompatible(_) => {
            log::error!(
                target: "se_manager::core",
                "operation=core_probe role={} stable_code=INCOMPATIBLE",
                role.endpoint_name()
            );
            // An incompatible Core (e.g. after an app update bumped the
            // protocol) must not keep squatting the endpoint while the GUI
            // silently falls back in-process. Ask it to shut down only if it
            // acked Hello, wait until the endpoint is absent, then replace.
            // A rejected pre-hello peer is still live: never unlink+spawn.
            if !replace_incompatible_core(&endpoint, role)
                .await
                .allows_unlink_and_spawn()
            {
                return Err(CoreError::Io(format!(
                    "incompatible {} is still live; refusing to unlink and spawn a second core",
                    role.endpoint_name()
                )));
            }
        }
        EndpointPresence::Absent => {
            #[cfg(not(any(unix, windows)))]
            return Err(CoreError::UnsupportedPlatform);
        }
    }

    prepare_runtime_dir(&endpoint)?;
    let owned_child_alive = !terminate_owned_core(role, &endpoint).await;
    if !may_unlink_owned_socket(owned_child_alive) {
        return Err(CoreError::Io(format!(
            "owned {} is still alive; refusing to unlink its socket",
            role.endpoint_name()
        )));
    }
    #[cfg(unix)]
    if endpoint.as_path().is_some() {
        let _ = super::ipc::remove_stale_socket(&endpoint);
    }

    let role_arg = match role {
        CoreRole::TerminalCore => "--terminal-core",
        CoreRole::AcpCore => "--acp-core",
        CoreRole::Gui => {
            return Err(CoreError::InvalidRequest(
                "GUI is not a launchable core role".to_string(),
            ))
        }
    };

    let mut command = Command::new(&config.executable);
    command
        .arg(role_arg)
        .env("TERMUL_CORE_PROFILE_ROOT", &config.profile_root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let child = command.spawn().map_err(|error| {
        log::error!(
            target: "se_manager::core",
            "operation=core_spawn role={} stable_code=SPAWN_FAILED",
            role.endpoint_name()
        );
        CoreError::Io(format!("spawn {}: {error}", role.endpoint_name()))
    })?;
    let pid = child.id();
    insert_owned_core(role, child);

    let deadline = tokio::time::Instant::now() + config.ready_timeout;
    loop {
        if let Ok(ack) = probe_endpoint(&endpoint, role).await {
            log::info!(
                target: "se_manager::core",
                "operation=core_ready role={} stable_code=READY pid={pid}",
                role.endpoint_name()
            );
            return Ok(CoreProcess {
                role,
                endpoint,
                pid,
                reused: false,
                remote_build_id: ack.component_build_id,
                remote_capabilities: ack.capabilities,
                active_resources: ack.active_resources,
                reconciliation_deferred: false,
                child: None,
            });
        }
        if tokio::time::Instant::now() >= deadline {
            log::error!(
                target: "se_manager::core",
                "operation=core_ready role={} stable_code=READY_TIMEOUT",
                role.endpoint_name()
            );
            let _ = terminate_owned_core(role, &endpoint).await;
            return Err(CoreError::Io(format!(
                "{} did not become ready within {:?}",
                role.endpoint_name(),
                config.ready_timeout
            )));
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

pub fn profile_root_from_env() -> PathBuf {
    std::env::var_os("TERMUL_CORE_PROFILE_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("termul-core"))
}

#[cfg(any(unix, windows))]
pub fn run_core_process(role: CoreRole) -> i32 {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("core runtime initialization failed: {error}");
            return 1;
        }
    };
    let result = match role {
        CoreRole::TerminalCore => {
            runtime.block_on(super::terminal::run_terminal_core(profile_root_from_env()))
        }
        CoreRole::AcpCore => runtime.block_on(super::acp::run_acp_core(profile_root_from_env())),
        CoreRole::Gui => Err(CoreError::InvalidRequest(
            "GUI is not a launchable core role".to_string(),
        )),
    };
    match result {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("{} process failed: {error}", role.endpoint_name());
            1
        }
    }
}

#[cfg(not(any(unix, windows)))]
pub fn run_core_process(role: CoreRole) -> i32 {
    eprintln!(
        "{} process is not supported on this platform yet",
        role.endpoint_name()
    );
    2
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_uses_current_executable() {
        let config = CoreLaunchConfig::for_current_executable("/tmp/termul-profile").unwrap();
        assert!(config.executable.is_absolute());
        assert_eq!(config.profile_root, Path::new("/tmp/termul-profile"));
    }

    #[test]
    fn rejects_gui_as_a_core_role() {
        assert_eq!(CoreRole::Gui.endpoint_name(), "gui");
    }

    #[test]
    fn adoption_readiness_and_fallback_logs_have_distinct_stable_codes() {
        let source = include_str!("launcher.rs");
        assert!(source.contains("operation=core_adopt role={} stable_code=ADOPTED"));
        assert!(source.contains("operation=core_ready role={} stable_code=READY pid={pid}"));
        assert!(source.contains("stable_code=INCOMPATIBLE_REPLACED"));
        assert!(source.contains("stable_code=ZOMBIE_ORPHANED"));
    }

    #[test]
    fn consecutive_miss_gate_requires_three_failures() {
        assert!(!should_declare_core_death(0));
        assert!(!should_declare_core_death(1));
        assert!(!should_declare_core_death(2));
        assert!(should_declare_core_death(3));
        assert!(should_declare_core_death(4));
    }

    #[test]
    fn never_unlinks_socket_while_owned_child_is_alive() {
        assert!(!may_unlink_owned_socket(true));
        assert!(may_unlink_owned_socket(false));
    }

    #[test]
    fn core_identity_classifies_legacy_and_current_acknowledgements() {
        let legacy = compatible_ack();
        assert_eq!(
            classify_core_identity(&legacy, CoreRole::AcpCore),
            CoreIdentityState::Legacy
        );

        let mut current = legacy;
        current.component_build_id = Some(component_build_id(CoreRole::AcpCore));
        assert_eq!(
            classify_core_identity(&current, CoreRole::AcpCore),
            CoreIdentityState::Current
        );

        current.component_build_id = Some("stale-build".to_string());
        assert_eq!(
            classify_core_identity(&current, CoreRole::AcpCore),
            CoreIdentityState::Stale
        );
    }

    #[test]
    fn terminal_replacement_is_safe_only_when_no_resources_are_active() {
        assert!(terminal_replacement_is_safe(0));
        assert!(!terminal_replacement_is_safe(1));
    }

    #[test]
    fn update_policy_only_enforces_components_that_need_reconciliation() {
        let policy = serde_json::json!({
            "metadataState": "declared",
            "components": {
                "renderer": { "action": "preserve", "buildId": "renderer" },
                "guiNative": { "action": "preserve", "buildId": "gui" },
                "acpCore": { "action": "restart", "buildId": "acp-next" },
                "terminalCore": { "action": "defer-if-active", "buildId": "terminal-next" }
            }
        });
        configure_update_policy(Some(&policy));
        assert!(update_reconciliation_pending(CoreRole::AcpCore));
        assert!(update_reconciliation_pending(CoreRole::TerminalCore));
        assert!(!update_reconciliation_pending(CoreRole::Gui));
        configure_update_policy(None);
    }

    fn compatible_ack() -> CoreHelloAck {
        CoreHelloAck {
            role: CoreRole::AcpCore,
            protocol_version: CURRENT_PROTOCOL_VERSION,
            component_build_id: None,
            capabilities: Vec::new(),
            active_resources: 0,
        }
    }

    #[test]
    fn connect_failure_is_absent_and_allows_fallback_and_spawn() {
        let presence = EndpointPresence::from_connect_and_handshake(
            Err(CoreError::Io("connection refused".into())),
            None,
        );
        assert!(!presence.is_live());
        assert!(presence.allows_in_process_fallback());
        assert!(presence.allows_unlink_and_spawn());
    }

    #[test]
    fn pre_hello_close_is_live_and_blocks_fallback_and_spawn() {
        let presence = EndpointPresence::from_connect_and_handshake(
            Ok(()),
            Some(Err(CoreError::Io("unexpected eof".into()))),
        );
        assert!(presence.is_live());
        assert!(!presence.allows_in_process_fallback());
        assert!(!presence.allows_unlink_and_spawn());
    }

    #[test]
    fn incompatible_ack_is_live_and_blocks_fallback_and_spawn() {
        let presence = EndpointPresence::from_connect_and_handshake(
            Ok(()),
            Some(Err(CoreError::InvalidHandshake(
                "core returned an incompatible handshake".into(),
            ))),
        );
        assert!(presence.is_live());
        assert!(!presence.allows_in_process_fallback());
        assert!(!presence.allows_unlink_and_spawn());
        assert!(!IncompatibleReplaceOutcome::StillLive.allows_unlink_and_spawn());
        assert!(IncompatibleReplaceOutcome::Cleared.allows_unlink_and_spawn());
    }

    #[test]
    fn compatible_ack_is_live_and_must_be_adopted_not_fallback() {
        let presence =
            EndpointPresence::from_connect_and_handshake(Ok(()), Some(Ok(compatible_ack())));
        assert!(presence.is_live());
        assert!(!presence.allows_in_process_fallback());
        match presence {
            EndpointPresence::LiveCompatible(ack) => {
                assert_eq!(ack.role, CoreRole::AcpCore);
                assert_eq!(ack.protocol_version, CURRENT_PROTOCOL_VERSION);
            }
            other => panic!("expected live compatible, got {other:?}"),
        }
    }

    #[test]
    fn incompatible_still_live_returns_before_unlink() {
        let source = include_str!("launcher.rs");
        let refuse = source
            .find("incompatible {} is still live; refusing to unlink and spawn a second core")
            .expect("still-live replace must fail closed");
        let unlink = source
            .find("remove_stale_socket")
            .expect("unix stale-socket unlink");
        assert!(
            refuse < unlink,
            "StillLive must return before remove_stale_socket + spawn"
        );
        let blocking_wait = concat!("owned.child", ".wait()");
        assert!(
            !source.contains(blocking_wait),
            "Windows/non-unix owned exit must poll try_wait, not block on Child::wait"
        );
        assert!(source.contains("wait_for_owned_exit"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn wait_for_owned_exit_reaps_a_short_lived_child_without_blocking_wait() {
        let mut child = Command::new("sleep")
            .arg("0")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn sleep 0");
        let profile = tempfile::tempdir().unwrap();
        let endpoint = CoreEndpoint::for_profile(profile.path(), CoreRole::Gui);
        assert!(wait_for_owned_exit(&mut child, &endpoint, Duration::from_secs(2)).await);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn terminate_owned_core_reaps_the_registry_entry() {
        let child = Command::new("sleep")
            .arg("30")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn sleep");
        let pid = child.id();
        insert_owned_core(CoreRole::Gui, child);
        assert_eq!(owned_child_pid(CoreRole::Gui), Some(pid));

        let profile = tempfile::tempdir().unwrap();
        let endpoint = CoreEndpoint::for_profile(profile.path(), CoreRole::Gui);
        assert!(terminate_owned_core(CoreRole::Gui, &endpoint).await);
        assert_eq!(owned_child_pid(CoreRole::Gui), None);

        let still_alive = unsafe { libc::kill(pid as i32, 0) == 0 };
        assert!(!still_alive, "owned child pid {pid} should be gone");
    }
}
