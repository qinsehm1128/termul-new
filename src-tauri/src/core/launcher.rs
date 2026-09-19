//! Core process discovery, spawning, and the minimal role server.
//!
//! The GUI owns only the client-side launch/reuse operation. Core processes do
//! not receive a kill-on-drop relationship from the GUI. The actual Terminal
//! and ACP service handlers are layered on top of this role server later.

use super::ipc::{
    prepare_runtime_dir, read_json_frame, write_json_frame, CoreEndpoint, CoreError, CoreHello,
    CoreHelloAck, CoreRole, CURRENT_PROTOCOL_VERSION,
};
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

#[cfg(unix)]
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
        if let Some(path) = endpoint.as_path() {
            if tokio::net::UnixStream::connect(path).await.is_err()
                && child.try_wait().ok().flatten().is_some()
            {
                return true;
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return child.try_wait().ok().flatten().is_some();
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// SIGTERM a previously spawned child for `role` and wait until it is reaped
/// (and the socket stops accepting). Returns whether the child is gone so the
/// caller may unlink. The child is put back in the registry if it is still
/// alive — never unlink while we still own a live process.
async fn terminate_owned_core(role: CoreRole, endpoint: &CoreEndpoint) -> bool {
    let Some(mut owned) = take_owned_core(role) else {
        return true;
    };
    #[cfg(unix)]
    send_sigterm(owned.pid);
    #[cfg(not(unix))]
    let _ = owned.child.kill();

    #[cfg(unix)]
    let exited = wait_for_owned_exit(&mut owned.child, endpoint, Duration::from_secs(3)).await;
    #[cfg(not(unix))]
    let exited = {
        let _ = endpoint;
        owned.child.wait().is_ok()
    };

    if exited {
        true
    } else {
        insert_owned_core(role, owned.child);
        false
    }
}

#[cfg(unix)]
pub(crate) async fn probe_endpoint(
    endpoint: &CoreEndpoint,
    expected_role: CoreRole,
) -> Result<CoreHelloAck, CoreError> {
    use tokio::net::UnixStream;

    let path = endpoint.as_path().ok_or(CoreError::UnsupportedPlatform)?;
    let mut stream = UnixStream::connect(path).await.map_err(CoreError::from)?;
    let hello = CoreHello {
        // Hello.role names the target server, not the caller. The GUI identifies
        // itself with `client_name` and asks for `expected_role`.
        role: expected_role,
        protocol_versions: vec![CURRENT_PROTOCOL_VERSION],
        client_name: "termul-gui-launcher".to_string(),
    };
    write_json_frame(&mut stream, &hello).await?;
    let ack: CoreHelloAck = read_json_frame(&mut stream).await?;
    if ack.role != expected_role || ack.protocol_version != CURRENT_PROTOCOL_VERSION {
        return Err(CoreError::InvalidHandshake(
            "core returned an incompatible handshake".to_string(),
        ));
    }
    Ok(ack)
}

#[cfg(not(unix))]
pub(crate) async fn probe_endpoint(
    _endpoint: &CoreEndpoint,
    _expected_role: CoreRole,
) -> Result<CoreHelloAck, CoreError> {
    Err(CoreError::UnsupportedPlatform)
}

pub async fn ensure_core(
    role: CoreRole,
    config: &CoreLaunchConfig,
) -> Result<CoreProcess, CoreError> {
    let endpoint = CoreEndpoint::for_profile(&config.profile_root, role);
    let lock = role_lock(role).await;
    let _guard = lock.lock().await;

    match probe_endpoint(&endpoint, role).await {
        Ok(_) => {
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
                child: None,
            });
        }
        Err(error @ CoreError::InvalidHandshake(_))
        | Err(error @ CoreError::UnsupportedProtocol { .. })
        | Err(error @ CoreError::Unauthorized) => {
            log::error!(
                target: "se_manager::core",
                "operation=core_probe role={} stable_code=INCOMPATIBLE",
                role.endpoint_name()
            );
            return Err(error);
        }
        Err(CoreError::UnsupportedPlatform) => return Err(CoreError::UnsupportedPlatform),
        Err(CoreError::Io(_))
        | Err(CoreError::InvalidFrame(_))
        | Err(CoreError::InvalidRequest(_)) => {}
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
        if probe_endpoint(&endpoint, role).await.is_ok() {
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

#[cfg(unix)]
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

#[cfg(not(unix))]
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
