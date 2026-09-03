//! SSH reverse tunnel (`ssh -N -R`) against an operator-owned VPS.
//!
//! SSH is only the pipe. Termul still requires the pairing bearer on the
//! published Origin. The remote bind is `127.0.0.1:{remotePort}` so the VPS
//! must terminate TLS (Caddy/Nginx) rather than exposing GatewayPorts.

use std::time::Duration;

use tokio::process::Command;

use super::config::{TunnelConfig, TunnelConfigStore, TunnelProviderKind};
use super::StartedTunnel;
use crate::acp::atomic_file;

const READY_GRACE: Duration = Duration::from_millis(1500);

pub async fn start_ssh_reverse_tunnel(
    local_port: u16,
    config: &TunnelConfig,
    store: &TunnelConfigStore,
) -> Result<StartedTunnel, String> {
    config.validate_for_start()?;
    let url = config.public_origin()?;
    let host = required_field(config.ssh_host.as_deref(), "SSH host")?;
    let user = required_field(config.ssh_user.as_deref(), "SSH user")?;
    let ssh_port = config.ssh_port.unwrap_or(22);
    let remote_port = config
        .ssh_remote_port
        .filter(|port| *port > 0)
        .ok_or_else(|| "SSH remote port is required".to_string())?;

    let key_path = if let Some(key) = store.ssh_private_key()? {
        Some(write_identity_file(store, &key)?)
    } else {
        None
    };

    let mut args: Vec<String> = vec![
        "-N".into(),
        "-o".into(),
        "ExitOnForwardFailure=yes".into(),
        "-o".into(),
        "ServerAliveInterval=30".into(),
        "-o".into(),
        "ServerAliveCountMax=3".into(),
        "-o".into(),
        "BatchMode=yes".into(),
        "-o".into(),
        "StrictHostKeyChecking=accept-new".into(),
        "-p".into(),
        ssh_port.to_string(),
        "-R".into(),
        format!("127.0.0.1:{remote_port}:127.0.0.1:{local_port}"),
    ];
    if let Some(path) = key_path.as_ref() {
        args.push("-o".into());
        args.push("IdentitiesOnly=yes".into());
        args.push("-i".into());
        args.push(path.to_string_lossy().into_owned());
    }
    args.push(format!("{user}@{host}"));

    log::info!(
        target: "se_manager::remote::tunnel",
        "operation=tunnel_start provider=sshReverse local_port={local_port} stable_code=OK"
    );

    let mut command = Command::new("ssh");
    command
        .args(&args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    crate::remote::cloudflared::strip_proxy_env(&mut command);
    crate::remote::cloudflared::configure_background_command(&mut command);

    let mut child = command.spawn().map_err(|error| {
        log::error!(
            target: "se_manager::remote::tunnel",
            "operation=tunnel_sidecar_spawn provider=sshReverse stable_code=SPAWN_FAILED"
        );
        format!("ssh reverse tunnel failed to spawn: {error}")
    })?;

    tokio::time::sleep(READY_GRACE).await;
    match child.try_wait() {
        Ok(Some(status)) => {
            return Err(format!(
                "ssh reverse tunnel exited before the forward was ready ({status})"
            ));
        }
        Ok(None) => {}
        Err(error) => return Err(format!("ssh reverse tunnel wait failed: {error}")),
    }

    log::info!(
        target: "se_manager::remote::tunnel",
        "operation=tunnel_ready provider=sshReverse stable_code=OK"
    );
    Ok(StartedTunnel {
        url,
        child,
        provider: TunnelProviderKind::SshReverse,
    })
}

fn required_field<'a>(value: Option<&'a str>, label: &str) -> Result<&'a str, String> {
    value
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("{label} is required"))
}

fn write_identity_file(store: &TunnelConfigStore, key: &str) -> Result<std::path::PathBuf, String> {
    let path = store.parent_dir().join("ssh-reverse-identity");
    let mut material = key.to_string();
    if !material.ends_with('\n') {
        material.push('\n');
    }
    atomic_file::replace(&path, material.as_bytes())
        .map_err(|error| format!("failed to write SSH identity: {error}"))?;
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
