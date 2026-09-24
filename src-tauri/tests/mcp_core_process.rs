use std::{
    net::{IpAddr, Ipv4Addr, TcpListener},
    path::PathBuf,
    time::Duration,
};

use se_manager_lib::mcp_core::{AuthBootstrap, McpCoreProcessConfig, McpCoreSupervisor};

fn free_port() -> u16 {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    listener.local_addr().unwrap().port()
}

fn config() -> McpCoreProcessConfig {
    McpCoreProcessConfig {
        executable: PathBuf::from(env!("CARGO_BIN_EXE_se-manager")),
        bind_address: IpAddr::V4(Ipv4Addr::LOCALHOST),
        port: free_port(),
        auth: AuthBootstrap::new(1, "process-test-token").unwrap(),
        request_body_limit: 64 * 1024,
        startup_timeout: Duration::from_secs(5),
    }
}

#[cfg(unix)]
#[tokio::test]
async fn shutdown_does_not_kill_an_unrelated_process() {
    let mut unrelated = tokio::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .unwrap();
    let mut supervisor = McpCoreSupervisor::new(config());
    supervisor.start().await.unwrap();
    supervisor.shutdown().await.unwrap();
    assert!(unrelated.try_wait().unwrap().is_none());
    unrelated.kill().await.unwrap();
    unrelated.wait().await.unwrap();
}

#[tokio::test]
async fn owned_mcp_core_starts_restarts_after_probe_misses_and_shuts_down_cleanly() {
    let mut supervisor = McpCoreSupervisor::new(config());
    supervisor.start().await.unwrap();
    assert_eq!(supervisor.consecutive_misses(), 0);
    assert!(supervisor.probe().await.unwrap());
    let status = supervisor.status().await.unwrap();
    assert_eq!(status.state, se_manager_lib::mcp_core::McpReadiness::Ready);
    assert_eq!(status.endpoint.unwrap().auth_generation, 1);

    supervisor.restart().await.unwrap();
    assert!(supervisor.probe().await.unwrap());

    supervisor.shutdown().await.unwrap();
    assert!(supervisor.probe().await.is_err());
    assert!(supervisor.probe().await.is_err());
    assert!(supervisor.probe().await.unwrap());

    let endpoint = supervisor.endpoint().to_owned();
    supervisor.shutdown().await.unwrap();
    let error = reqwest::get(endpoint).await.unwrap_err();
    assert!(error.is_connect() || error.is_request());
}
