//! Non-Tauri MCP Core process role and owned-child supervisor.
//!
//! This lifecycle is intentionally separate from Terminal Core/ACP Core. The
//! supervisor only ever terminates a child it spawned itself.

use std::{
    net::{IpAddr, Ipv4Addr},
    path::PathBuf,
    process::Stdio,
    time::Duration,
};

use tokio::{
    process::{Child, Command},
    time::Instant,
};

use super::{
    AuthBootstrap, BuiltInRegistry, McpCore, McpCoreConfig, McpHttpGateway, McpHttpGatewayConfig,
    McpReadinessStatus,
};
use crate::memory_index::service::MemoryIndexService;

pub const MCP_CORE_BIND_ENV: &str = "TERMUL_MCP_CORE_BIND";
pub const MCP_CORE_PORT_ENV: &str = "TERMUL_MCP_CORE_PORT";
pub const MCP_CORE_AUTH_TOKEN_ENV: &str = "TERMUL_MCP_CORE_AUTH_TOKEN";
pub const MCP_CORE_AUTH_GENERATION_ENV: &str = "TERMUL_MCP_CORE_AUTH_GENERATION";
pub const MCP_CORE_REQUEST_LIMIT_ENV: &str = "TERMUL_MCP_CORE_REQUEST_LIMIT";
pub const MCP_CORE_PARENT_PID_ENV: &str = "TERMUL_MCP_CORE_PARENT_PID";
/// Optional host-private memory-index state root used by the independent Core
/// process. The HTTP caller still selects and authorizes its project per call.
pub const MCP_CORE_MEMORY_STATE_ROOT_ENV: &str = "TERMUL_MCP_CORE_MEMORY_STATE_ROOT";
pub const MCP_CORE_PROBE_MISSES: u32 = 3;

#[derive(Debug, Clone)]
pub struct McpCoreProcessConfig {
    pub executable: PathBuf,
    pub bind_address: IpAddr,
    pub port: u16,
    pub auth: AuthBootstrap,
    pub request_body_limit: usize,
    pub startup_timeout: Duration,
}

impl McpCoreProcessConfig {
    pub fn from_env(executable: impl Into<PathBuf>) -> Result<Self, ProcessError> {
        let bind_address = std::env::var(MCP_CORE_BIND_ENV)
            .ok()
            .map(|value| value.parse())
            .transpose()
            .map_err(|_| ProcessError::InvalidConfig("bind address is invalid".into()))?
            .unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST));
        if !bind_address.is_loopback() {
            return Err(ProcessError::InvalidConfig(
                "MCP Core process must bind to loopback".into(),
            ));
        }
        let port = required_env_u16(MCP_CORE_PORT_ENV)?;
        if port == 0 {
            return Err(ProcessError::InvalidConfig(
                "MCP Core process port must be assigned".into(),
            ));
        }
        let token = std::env::var(MCP_CORE_AUTH_TOKEN_ENV)
            .map_err(|_| ProcessError::InvalidConfig("MCP Core auth token is missing".into()))?;
        let generation = std::env::var(MCP_CORE_AUTH_GENERATION_ENV)
            .ok()
            .map(|value| value.parse())
            .transpose()
            .map_err(|_| ProcessError::InvalidConfig("MCP Core auth generation is invalid".into()))?
            .unwrap_or(1);
        let request_body_limit = std::env::var(MCP_CORE_REQUEST_LIMIT_ENV)
            .ok()
            .map(|value| value.parse())
            .transpose()
            .map_err(|_| ProcessError::InvalidConfig("MCP Core request limit is invalid".into()))?
            .unwrap_or(8 * 1024 * 1024);
        Ok(Self {
            executable: executable.into(),
            bind_address,
            port,
            auth: AuthBootstrap::new(generation, token)
                .map_err(|error| ProcessError::InvalidConfig(error.message))?,
            request_body_limit,
            startup_timeout: Duration::from_secs(5),
        })
    }
}

fn required_env_u16(name: &str) -> Result<u16, ProcessError> {
    std::env::var(name)
        .map_err(|_| ProcessError::InvalidConfig(format!("{name} is missing")))?
        .parse()
        .map_err(|_| ProcessError::InvalidConfig(format!("{name} is invalid")))
}

#[derive(Debug)]
pub enum ProcessError {
    InvalidConfig(String),
    Spawn(std::io::Error),
    Probe(reqwest::Error),
    ReadinessTimeout,
    Child(std::io::Error),
}

impl std::fmt::Display for ProcessError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidConfig(message) => write!(f, "invalid MCP Core process config: {message}"),
            Self::Spawn(error) => write!(f, "failed to spawn MCP Core: {error}"),
            Self::Probe(error) => write!(f, "MCP Core readiness probe failed: {error}"),
            Self::ReadinessTimeout => f.write_str("MCP Core did not become ready before timeout"),
            Self::Child(error) => write!(f, "MCP Core child operation failed: {error}"),
        }
    }
}

impl std::error::Error for ProcessError {}

pub struct McpCoreSupervisor {
    config: McpCoreProcessConfig,
    endpoint: String,
    status_endpoint: String,
    client: reqwest::Client,
    child: Option<Child>,
    consecutive_misses: u32,
}

impl std::fmt::Debug for McpCoreSupervisor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpCoreSupervisor")
            .field("endpoint", &self.endpoint)
            .field("owned_child", &self.child.is_some())
            .field("consecutive_misses", &self.consecutive_misses)
            .finish_non_exhaustive()
    }
}

impl McpCoreSupervisor {
    pub fn new(config: McpCoreProcessConfig) -> Self {
        let base = format!("http://{}:{}", config.bind_address, config.port);
        Self {
            config,
            endpoint: format!("{base}/ready"),
            status_endpoint: format!("{base}/mcp-core/status"),
            client: reqwest::Client::new(),
            child: None,
            consecutive_misses: 0,
        }
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub fn consecutive_misses(&self) -> u32 {
        self.consecutive_misses
    }

    pub async fn start(&mut self) -> Result<(), ProcessError> {
        if self.child.is_some() {
            return Ok(());
        }
        self.spawn_owned().await?;
        let deadline = Instant::now() + self.config.startup_timeout;
        loop {
            if matches!(self.probe_ready().await, Ok(true)) {
                self.consecutive_misses = 0;
                return Ok(());
            }
            if Instant::now() >= deadline {
                self.stop_owned().await?;
                return Err(ProcessError::ReadinessTimeout);
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    pub async fn status(&self) -> Result<McpReadinessStatus, ProcessError> {
        self.client
            .get(&self.status_endpoint)
            .send()
            .await
            .map_err(ProcessError::Probe)?
            .error_for_status()
            .map_err(ProcessError::Probe)?
            .json()
            .await
            .map_err(ProcessError::Probe)
    }

    pub async fn probe(&mut self) -> Result<bool, ProcessError> {
        match self.probe_ready().await {
            Ok(true) => {
                self.consecutive_misses = 0;
                Ok(true)
            }
            Ok(false) => {
                self.consecutive_misses = self.consecutive_misses.saturating_add(1);
                if self.consecutive_misses >= MCP_CORE_PROBE_MISSES {
                    self.restart().await?;
                    return Ok(true);
                }
                Ok(false)
            }
            Err(error) => {
                self.consecutive_misses = self.consecutive_misses.saturating_add(1);
                if self.consecutive_misses >= MCP_CORE_PROBE_MISSES {
                    self.restart().await?;
                    return Ok(true);
                }
                Err(error)
            }
        }
    }

    pub async fn restart(&mut self) -> Result<(), ProcessError> {
        self.stop_owned().await?;
        self.consecutive_misses = 0;
        self.start().await
    }

    pub async fn shutdown(&mut self) -> Result<(), ProcessError> {
        self.stop_owned().await
    }

    async fn probe_ready(&self) -> Result<bool, ProcessError> {
        let response = self
            .client
            .get(&self.endpoint)
            .send()
            .await
            .map_err(ProcessError::Probe)?;
        Ok(response.status().is_success())
    }

    async fn spawn_owned(&mut self) -> Result<(), ProcessError> {
        let mut command = Command::new(&self.config.executable);
        command
            .arg("--mcp-core")
            .env(MCP_CORE_BIND_ENV, self.config.bind_address.to_string())
            .env(MCP_CORE_PORT_ENV, self.config.port.to_string())
            .env(MCP_CORE_AUTH_TOKEN_ENV, self.config.auth.bearer_token())
            .env(
                MCP_CORE_AUTH_GENERATION_ENV,
                self.config.auth.generation.to_string(),
            )
            .env(
                MCP_CORE_REQUEST_LIMIT_ENV,
                self.config.request_body_limit.to_string(),
            )
            .env(MCP_CORE_PARENT_PID_ENV, std::process::id().to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        self.child = Some(command.spawn().map_err(ProcessError::Spawn)?);
        Ok(())
    }

    async fn stop_owned(&mut self) -> Result<(), ProcessError> {
        let Some(mut child) = self.child.take() else {
            return Ok(());
        };
        if child.try_wait().map_err(ProcessError::Child)?.is_none() {
            child.kill().await.map_err(ProcessError::Child)?;
        }
        child.wait().await.map_err(ProcessError::Child)?;
        Ok(())
    }
}

async fn wait_for_parent_disconnect(parent_pid: Option<u32>) {
    let Some(parent_pid) = parent_pid else {
        std::future::pending::<()>().await;
        return;
    };
    loop {
        tokio::time::sleep(Duration::from_millis(250)).await;
        if !parent_process_is_alive(parent_pid) {
            return;
        }
    }
}

#[cfg(unix)]
fn parent_process_is_alive(pid: u32) -> bool {
    // A permission error still means the process exists; only ESRCH means the
    // supervisor has gone away.
    let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(not(unix))]
fn parent_process_is_alive(_pid: u32) -> bool {
    true
}

pub fn run_mcp_core_process() -> i32 {
    let executable = match std::env::current_exe() {
        Ok(executable) => executable,
        Err(error) => {
            eprintln!("MCP Core executable resolution failed: {error}");
            return 1;
        }
    };
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("MCP Core runtime initialization failed: {error}");
            return 1;
        }
    };
    runtime.block_on(async move {
        let process = match McpCoreProcessConfig::from_env(executable) {
            Ok(process) => process,
            Err(error) => {
                eprintln!("{error}");
                return 2;
            }
        };
        let core = std::env::var_os(MCP_CORE_MEMORY_STATE_ROOT_ENV)
            .map(PathBuf::from)
            .map(|state_root| {
                McpCore::new_with_builtins(
                    McpCoreConfig::default(),
                    std::sync::Arc::new(super::AllowAllTools),
                    BuiltInRegistry::memory_backed(
                        std::sync::Arc::new(MemoryIndexService::new(state_root)),
                        None,
                    ),
                )
            })
            .unwrap_or_default();
        let gateway = match McpHttpGateway::bind(
            std::sync::Arc::new(core),
            McpHttpGatewayConfig {
                bind_address: process.bind_address,
                port: process.port,
                path: "/mcp".into(),
                generation: process.auth.generation,
                auth: process.auth,
                request_body_limit: process.request_body_limit,
            },
        )
        .await
        {
            Ok(gateway) => gateway,
            Err(error) => {
                eprintln!("MCP Core gateway failed: {error}");
                return 1;
            }
        };
        let parent_pid = std::env::var(MCP_CORE_PARENT_PID_ENV)
            .ok()
            .and_then(|value| value.parse().ok());
        tokio::select! {
            result = tokio::signal::ctrl_c() => {
                if let Err(error) = result {
                    eprintln!("MCP Core shutdown signal failed: {error}");
                }
            }
            _ = wait_for_parent_disconnect(parent_pid) => {}
        }
        gateway.shutdown().await;
        0
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_consecutive_misses_is_the_restart_threshold() {
        assert_eq!(MCP_CORE_PROBE_MISSES, 3);
    }

    #[test]
    fn process_config_debug_output_does_not_expose_credentials() {
        let process = McpCoreProcessConfig {
            executable: PathBuf::from("/tmp/se-manager"),
            bind_address: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port: 1234,
            auth: AuthBootstrap::new(7, "secret-token").unwrap(),
            request_body_limit: 1024,
            startup_timeout: Duration::from_secs(1),
        };
        assert!(!format!("{process:?}").contains("secret-token"));
    }
}
