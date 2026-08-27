//! Operator-owned FRP client (`frpc`) against a self-hosted `frps`.
//!
//! The TOML is written under the app-data tunnel directory with `0600` on Unix.
//! The auth token is interpolated into that file for frpc, then the file is
//! best-effort shredded on stop by overwriting via the next start. The token
//! is never logged.

use std::time::Duration;

use lazy_static::lazy_static;
use regex::Regex;

use super::config::{TunnelConfig, TunnelConfigStore, TunnelProviderKind};
use super::process::{spawn_sidecar, wait_for_ready};
use super::sidecar::detect_frpc_path;
use super::StartedTunnel;
use crate::acp::atomic_file;

const READY_TIMEOUT_SECS: u64 = 25;

lazy_static! {
    static ref FRP_READY_RE: Regex = Regex::new(
        r"(?i)(start proxy success|login to server success|proxy added|start proxy success)"
    )
    .expect("valid frp ready regex");
}

pub async fn start_frp_tunnel(
    local_port: u16,
    config: &TunnelConfig,
    store: &TunnelConfigStore,
) -> Result<StartedTunnel, String> {
    config.validate_for_start()?;
    let url = config.public_origin()?;
    let token = store.require_frp_token()?;
    let toml = render_frpc_toml(local_port, config, &token)?;
    let config_path = store_config_path(store, &toml)?;
    let path = detect_frpc_path();
    log::info!(
        target: "termul::remote::tunnel",
        "operation=tunnel_start provider=frp local_port={local_port} stable_code=OK"
    );

    let config_arg = config_path.to_string_lossy().into_owned();
    let mut child = spawn_sidecar(&path, &["-c", &config_arg], &[])?;

    if let Err(error) = wait_for_ready(
        &mut child,
        Duration::from_secs(READY_TIMEOUT_SECS),
        &FRP_READY_RE,
        true,
    )
    .await
    {
        let _ = child.kill().await;
        return Err(error);
    }

    log::info!(
        target: "termul::remote::tunnel",
        "operation=tunnel_ready provider=frp stable_code=OK"
    );
    Ok(StartedTunnel {
        url,
        child,
        provider: TunnelProviderKind::Frp,
    })
}

fn store_config_path(store: &TunnelConfigStore, toml: &str) -> Result<std::path::PathBuf, String> {
    let path = frpc_runtime_path(store);
    atomic_file::replace(&path, toml.as_bytes())
        .map_err(|e| format!("failed to write frpc config: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(&path) {
            let mut perms = meta.permissions();
            perms.set_mode(0o600);
            let _ = std::fs::set_permissions(&path, perms);
        }
    }
    Ok(path)
}

fn frpc_runtime_path(store: &TunnelConfigStore) -> std::path::PathBuf {
    // Sibling of config.json: <app_data>/remote-tunnel/frpc.toml
    store_parent(store).join("frpc.toml")
}

fn store_parent(store: &TunnelConfigStore) -> std::path::PathBuf {
    store.parent_dir()
}

pub fn render_frpc_toml(
    local_port: u16,
    config: &TunnelConfig,
    token: &str,
) -> Result<String, String> {
    let server_addr = config
        .frp_server_addr
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "FRP server address is required".to_string())?;
    let server_port = config.frp_server_port.unwrap_or(7000);
    let escaped_token = escape_toml_string(token);
    let mut out = format!(
        "serverAddr = \"{}\"\nserverPort = {server_port}\n\n[auth]\nmethod = \"token\"\ntoken = \"{escaped_token}\"\n\n[[proxies]]\nname = \"termul\"\nlocalIP = \"127.0.0.1\"\nlocalPort = {local_port}\n",
        escape_toml_string(server_addr)
    );
    if let Some(domain) = config
        .frp_custom_domain
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let host = super::config::normalize_hostname(domain)?;
        out.push_str("type = \"http\"\n");
        out.push_str(&format!(
            "customDomains = [\"{}\"]\n",
            escape_toml_string(&host)
        ));
    } else {
        let remote_port = config
            .frp_remote_port
            .filter(|p| *p > 0)
            .ok_or_else(|| "FRP remote port is required without a custom domain".to_string())?;
        out.push_str("type = \"tcp\"\n");
        out.push_str(&format!("remotePort = {remote_port}\n"));
    }
    Ok(out)
}

fn escape_toml_string(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::tunnel::config::TunnelConfigStore;

    #[test]
    fn ready_regex_matches_frp_logs() {
        assert!(FRP_READY_RE.is_match("[I] [service.go:123] login to server success"));
        assert!(FRP_READY_RE.is_match("start proxy success"));
        assert!(!FRP_READY_RE.is_match("try to connect to server"));
    }

    #[test]
    fn http_proxy_toml_uses_custom_domain() {
        let config = TunnelConfig {
            provider: TunnelProviderKind::Frp,
            frp_server_addr: Some("vps.example.com".to_string()),
            frp_server_port: Some(7000),
            frp_custom_domain: Some("termul.example.com".to_string()),
            ..TunnelConfig::default()
        };
        let toml = render_frpc_toml(18787, &config, "s3cret\"value").unwrap();
        assert!(toml.contains("serverAddr = \"vps.example.com\""));
        assert!(toml.contains("localPort = 18787"));
        assert!(toml.contains("type = \"http\""));
        assert!(toml.contains("customDomains = [\"termul.example.com\"]"));
        assert!(toml.contains("token = \"s3cret\\\"value\""));
        assert!(!toml.contains("remotePort"));
    }

    #[test]
    fn tcp_proxy_toml_uses_remote_port() {
        let config = TunnelConfig {
            provider: TunnelProviderKind::Frp,
            frp_server_addr: Some("1.2.3.4".to_string()),
            frp_remote_port: Some(8443),
            ..TunnelConfig::default()
        };
        let toml = render_frpc_toml(9000, &config, "tok").unwrap();
        assert!(toml.contains("type = \"tcp\""));
        assert!(toml.contains("remotePort = 8443"));
        assert!(toml.contains("localPort = 9000"));
    }

    #[test]
    fn store_parent_is_config_directory() {
        let dir = tempfile::tempdir().unwrap();
        let store = TunnelConfigStore::for_path(dir.path().join("config.json"));
        assert_eq!(store.parent_dir(), dir.path());
    }
}
