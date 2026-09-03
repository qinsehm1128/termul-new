//! Operator-owned Cloudflare named tunnel (`cloudflared tunnel run`).
//!
//! The public hostname is configured by the operator. The sidecar uses
//! `TUNNEL_TOKEN` (never argv) so the token is not visible in `ps`. Ingress
//! for remotely-managed tunnels must already target the configured local port
//! (default 18787).

use std::time::Duration;

use lazy_static::lazy_static;
use regex::Regex;

use super::config::{TunnelConfig, TunnelConfigStore, TunnelProviderKind};
use super::process::{spawn_sidecar, wait_for_ready};
use super::StartedTunnel;
use crate::remote::cloudflared::detect_cloudflared_path;

const READY_TIMEOUT_SECS: u64 = 25;

lazy_static! {
    static ref NAMED_READY_RE: Regex =
        Regex::new(r"(?i)(registered tunnel connection|connection .+ registered|connindex=)")
            .expect("valid named-tunnel ready regex");
}

pub async fn start_named_tunnel(
    local_port: u16,
    config: &TunnelConfig,
    store: &TunnelConfigStore,
) -> Result<StartedTunnel, String> {
    config.validate_for_start()?;
    let url = config.public_origin()?;
    let token = store.named_token()?;
    let path = detect_cloudflared_path();
    log::info!(
        target: "se_manager::remote::tunnel",
        "operation=tunnel_start provider=cloudflareNamed local_port={local_port} stable_code=OK"
    );

    let mut child = spawn_sidecar(&path, &["tunnel", "run"], &[("TUNNEL_TOKEN", token)])?;

    if let Err(error) = wait_for_ready(
        &mut child,
        Duration::from_secs(READY_TIMEOUT_SECS),
        &NAMED_READY_RE,
        true,
    )
    .await
    {
        let _ = child.kill().await;
        return Err(error);
    }

    log::info!(
        target: "se_manager::remote::tunnel",
        "operation=tunnel_ready provider=cloudflareNamed stable_code=OK"
    );
    Ok(StartedTunnel {
        url,
        child,
        provider: TunnelProviderKind::CloudflareNamed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ready_regex_matches_cloudflared_named_logs() {
        assert!(NAMED_READY_RE.is_match("INF Registered tunnel connection connIndex=0"));
        assert!(NAMED_READY_RE.is_match("Connection abc registered"));
        assert!(!NAMED_READY_RE.is_match("starting cloudflared"));
    }
}
