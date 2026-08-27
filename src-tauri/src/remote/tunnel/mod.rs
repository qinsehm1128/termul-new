//! Pluggable public-tunnel providers for desktop shared-live access.
//!
//! The in-process Axum server binds loopback (or all interfaces for LAN).
//! A provider then publishes that origin through Cloudflare Quick Tunnel,
//! a named Cloudflare tunnel, FRP, or SSH reverse (`ssh -R`).
//! Application auth stays on the bearer fragment; providers only supply the
//! public Origin.

pub mod cloudflare_named;
pub mod commands;
pub mod config;
pub mod frp;
mod process;
mod sidecar;
pub mod ssh_reverse;

use tokio::process::Child;

use crate::remote::cloudflared;

pub use config::{TunnelConfig, TunnelConfigStore, TunnelProviderKind};

/// A live tunnel: public Origin (no credential fragment) plus the sidecar child.
pub struct StartedTunnel {
    pub url: String,
    pub child: Child,
    pub provider: TunnelProviderKind,
}

/// Start the configured provider against an already-bound loopback port.
///
/// Secrets are read from the in-memory settings cache (`secrets.json`) and never returned.
pub async fn start_configured_tunnel(
    local_port: u16,
    config: &TunnelConfig,
    store: &TunnelConfigStore,
) -> Result<StartedTunnel, String> {
    match config.provider {
        TunnelProviderKind::CloudflareQuick => {
            let tunnel = cloudflared::start_quick_tunnel(local_port).await?;
            Ok(StartedTunnel {
                url: tunnel.url,
                child: tunnel.child,
                provider: TunnelProviderKind::CloudflareQuick,
            })
        }
        TunnelProviderKind::CloudflareNamed => {
            cloudflare_named::start_named_tunnel(local_port, config, store).await
        }
        TunnelProviderKind::Frp => frp::start_frp_tunnel(local_port, config, store).await,
        TunnelProviderKind::SshReverse => {
            ssh_reverse::start_ssh_reverse_tunnel(local_port, config, store).await
        }
    }
}
