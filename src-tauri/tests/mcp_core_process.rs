use std::{
    net::{Ipv4Addr, TcpListener, TcpStream},
    process::Stdio,
    time::Duration,
};

const REJECTION: &str =
    "MCP gateway requires --config; use se-mcp --source internal or --source aggregator.\n";

async fn assert_rejected(arguments: &[&str], invalid_environment: bool) {
    let profile = tempfile::tempdir().unwrap();
    let probe = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let address = probe.local_addr().unwrap();
    drop(probe);

    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_se-manager"));
    command
        .args(arguments)
        .env("HOME", profile.path())
        .env("USERPROFILE", profile.path())
        .env("XDG_CONFIG_HOME", profile.path())
        .env("APPDATA", profile.path())
        .env("TERMUL_MCP_CORE_BIND", "127.0.0.1")
        .env("TERMUL_MCP_CORE_PORT", address.port().to_string())
        .env("TERMUL_MCP_CORE_AUTH_TOKEN", "process-test-secret-sentinel")
        .env("TERMUL_MCP_CORE_AUTH_GENERATION", "1")
        .env("TERMUL_MCP_CORE_REQUEST_LIMIT", "65536")
        .env("TERMUL_MCP_CORE_PROFILE_ROOT", profile.path())
        .env(
            "TERMUL_MCP_CORE_REGISTRY",
            profile.path().join("absent.json"),
        )
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if invalid_environment {
        command
            .env("TERMUL_MCP_CORE_BIND", "not-an-address")
            .env("TERMUL_MCP_CORE_PORT", "not-a-port")
            .env("TERMUL_MCP_CORE_AUTH_GENERATION", "not-a-generation")
            .env("TERMUL_MCP_CORE_REQUEST_LIMIT", "not-a-limit");
    }

    let child = command.spawn().unwrap();
    let waiting = child.wait_with_output();
    tokio::pin!(waiting);
    let output = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            tokio::select! {
                result = &mut waiting => break result,
                _ = tokio::time::sleep(Duration::from_millis(5)) => {
                    assert!(
                        TcpStream::connect_timeout(&address, Duration::from_millis(10)).is_err(),
                        "retired child must never accept connections"
                    );
                }
            }
        }
    })
    .await
    .expect("retired child must exit immediately, not start a runtime")
    .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert_eq!(String::from_utf8(output.stderr).unwrap(), REJECTION);
    assert!(TcpStream::connect_timeout(&address, Duration::from_millis(100)).is_err());
    let _still_free = TcpListener::bind(address).expect("retired child must not leave a listener");
    assert_eq!(std::fs::read_dir(profile.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn env_configured_mcp_core_entry_is_rejected_without_opening_a_listener() {
    assert_rejected(&["--mcp-core"], false).await;
}

#[tokio::test]
async fn retired_mcp_core_ignores_even_invalid_environment_values() {
    assert_rejected(&["--mcp-core"], true).await;
}

#[tokio::test]
async fn missing_config_value_is_rejected_instead_of_falling_through_to_gui() {
    assert_rejected(&["--mcp-core", "--config"], false).await;
    assert_rejected(&["--mcp-core", "--config", "--terminal-core"], false).await;
}

#[test]
fn retired_process_module_contains_only_a_fixed_rejection_entrypoint() {
    let source = include_str!("../src/mcp_core/process.rs");
    assert_eq!(source.matches("pub fn ").count(), 1);
    assert!(source.contains("pub fn run_mcp_core_process() -> i32"));
    for forbidden in [
        "\nuse ",
        "std::env",
        "tokio",
        "TcpListener",
        "spawn",
        "supervisor",
    ] {
        assert!(
            !source.contains(forbidden),
            "retired module contains {forbidden}"
        );
    }
}
