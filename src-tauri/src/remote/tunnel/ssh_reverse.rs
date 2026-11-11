//! SSH reverse tunnel (`ssh -N -R`) against an operator-owned VPS.
//!
//! SSH is only the pipe. Se still requires the pairing bearer on the
//! published Origin. The remote bind is `0.0.0.0:{remotePort}` so a phone can
//! open `http://{host}:{remotePort}` without a TLS reverse proxy. sshd must
//! allow that bind (`GatewayPorts yes` or `clientspecified`).

use std::path::PathBuf;
use std::time::Duration;

use tokio::process::Command;

use super::config::{TunnelConfig, TunnelConfigStore, TunnelProviderKind};
use super::StartedTunnel;
use crate::acp::atomic_file;

const READY_GRACE: Duration = Duration::from_millis(1500);
/// Budget for confirming the published port answers from the internet.
const PUBLIC_PROBE_TIMEOUT: Duration = Duration::from_secs(10);
const PUBLIC_PROBE_INTERVAL: Duration = Duration::from_millis(500);

/// `/health` on the published Origin; it needs no pairing bearer.
fn public_health_url(origin: &str) -> String {
    format!("{}/health", origin.trim_end_matches('/'))
}

/// sshd accepts `-R 0.0.0.0:port` even with `GatewayPorts no` and silently
/// binds loopback instead, so the forward looks healthy while a phone gets
/// "connection refused". Name the server-side fix.
fn unreachable_error(origin: &str, remote_port: u16) -> String {
    format!(
        "SSH tunnel is connected, but {origin} is not reachable from the internet. \
         On the VPS, set `GatewayPorts clientspecified` (or `yes`) in sshd_config, reload sshd, \
         and allow TCP {remote_port} in the firewall or security group."
    )
}

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

    let identity_path = resolve_identity_path(config, store)?;
    let password = store.ssh_password()?;
    let askpass_path = if let Some(password) = password.as_deref() {
        Some(write_askpass(store, password)?)
    } else {
        clear_askpass(store);
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
        "StrictHostKeyChecking=accept-new".into(),
        "-p".into(),
        ssh_port.to_string(),
        "-R".into(),
        format!("0.0.0.0:{remote_port}:127.0.0.1:{local_port}"),
    ];
    if askpass_path.is_none() {
        args.push("-o".into());
        args.push("BatchMode=yes".into());
    } else {
        args.push("-o".into());
        args.push("NumberOfPasswordPrompts=1".into());
        args.push("-o".into());
        args.push("PreferredAuthentications=publickey,password,keyboard-interactive".into());
    }
    if let Some(path) = identity_path.as_ref() {
        args.push("-o".into());
        args.push("IdentitiesOnly=yes".into());
        args.push("-i".into());
        args.push(path.to_string_lossy().into_owned());
    }
    args.push(format!("{user}@{host}"));

    log::info!(
        target: "se_manager::remote::tunnel",
        "operation=tunnel_start provider=sshReverse local_port={local_port} auth={} stable_code=OK",
        if askpass_path.is_some() { "password" } else if identity_path.is_some() { "key" } else { "agent" }
    );

    let mut command = Command::new("ssh");
    command
        .args(&args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    if let Some(script) = askpass_path.as_ref() {
        command.env("SSH_ASKPASS", script);
        command.env("SSH_ASKPASS_REQUIRE", "force");
        command.env("DISPLAY", ":0");
        command.env_remove("SSH_ASKPASS_PROMPT");
    }
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

    // The ssh process staying up only proves the forward exists on the VPS,
    // not that it is published. Confirm the path the phone will use.
    if crate::remote::cloudflared::probe_tunnel_ready_with(
        &public_health_url(&url),
        PUBLIC_PROBE_TIMEOUT,
        PUBLIC_PROBE_INTERVAL,
    )
    .await
    .is_err()
    {
        let _ = child.start_kill();
        log::warn!(
            target: "se_manager::remote::tunnel",
            "operation=tunnel_public_probe provider=sshReverse stable_code=PUBLIC_UNREACHABLE"
        );
        return Err(unreachable_error(&url, remote_port));
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

fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\")) {
        if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(path)
}

fn required_field<'a>(value: Option<&'a str>, label: &str) -> Result<&'a str, String> {
    value
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("{label} is required"))
}

fn resolve_identity_path(
    config: &TunnelConfig,
    store: &TunnelConfigStore,
) -> Result<Option<PathBuf>, String> {
    if let Some(path) = config
        .ssh_identity_file
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let path = expand_tilde(path);
        if path.is_file() {
            return Ok(Some(path));
        }
        log::warn!(
            target: "se_manager::remote::tunnel",
            "operation=tunnel_ssh_identity stable_code=MISSING path_missing=1"
        );
    }
    if let Some(key) = store.ssh_private_key()? {
        return Ok(Some(write_identity_file(store, &key)?));
    }
    Ok(None)
}

fn write_identity_file(store: &TunnelConfigStore, key: &str) -> Result<PathBuf, String> {
    let path = store.parent_dir().join("ssh-reverse-identity");
    let mut material = key.to_string();
    if !material.ends_with('\n') {
        material.push('\n');
    }
    atomic_file::replace(&path, material.as_bytes())
        .map_err(|error| format!("failed to write SSH identity: {error}"))?;
    restrict_owner_only(&path);
    Ok(path)
}

fn write_askpass(store: &TunnelConfigStore, password: &str) -> Result<PathBuf, String> {
    let dir = store.parent_dir();
    let data_path = dir.join("ssh-reverse-askpass.dat");
    atomic_file::replace(&data_path, password.as_bytes())
        .map_err(|error| format!("failed to write SSH askpass data: {error}"))?;
    restrict_owner_only(&data_path);

    #[cfg(windows)]
    let script_path = {
        let path = dir.join("ssh-reverse-askpass.bat");
        let content = format!("@echo off\r\ntype \"{}\"\r\n", data_path.to_string_lossy());
        atomic_file::replace(&path, content.as_bytes())
            .map_err(|error| format!("failed to write SSH askpass: {error}"))?;
        path
    };

    #[cfg(unix)]
    let script_path = {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join("ssh-reverse-askpass.sh");
        let content = format!("#!/bin/sh\ncat \"{}\"\n", data_path.to_string_lossy());
        atomic_file::replace(&path, content.as_bytes())
            .map_err(|error| format!("failed to write SSH askpass: {error}"))?;
        if let Ok(meta) = std::fs::metadata(&path) {
            let mut perms = meta.permissions();
            perms.set_mode(0o700);
            let _ = std::fs::set_permissions(&path, perms);
        }
        path
    };

    Ok(script_path)
}

fn clear_askpass(store: &TunnelConfigStore) {
    let dir = store.parent_dir();
    let _ = std::fs::remove_file(dir.join("ssh-reverse-askpass.dat"));
    let _ = std::fs::remove_file(dir.join("ssh-reverse-askpass.sh"));
    let _ = std::fs::remove_file(dir.join("ssh-reverse-askpass.bat"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_url_is_on_the_published_origin() {
        assert_eq!(
            public_health_url("http://203.0.113.7:18787"),
            "http://203.0.113.7:18787/health"
        );
        assert_eq!(
            public_health_url("http://203.0.113.7:18787/"),
            "http://203.0.113.7:18787/health"
        );
    }

    #[test]
    fn unreachable_error_names_the_server_side_fix() {
        let message = unreachable_error("http://203.0.113.7:18787", 18787);
        assert!(message.contains("GatewayPorts clientspecified"));
        assert!(message.contains("18787"));
        assert!(message.contains("http://203.0.113.7:18787"));
    }

    #[tokio::test]
    async fn a_loopback_only_forward_is_reported_unreachable() {
        // Nothing listens on this port, as with a forward sshd bound to
        // loopback on the VPS: the probe must fail, not report ready.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let result = crate::remote::cloudflared::probe_tunnel_ready_with(
            &public_health_url(&format!("http://127.0.0.1:{port}")),
            Duration::from_millis(600),
            Duration::from_millis(100),
        )
        .await;
        assert!(result.is_err());
    }
}

fn restrict_owner_only(path: &PathBuf) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(path) {
            let mut perms = meta.permissions();
            perms.set_mode(0o600);
            let _ = std::fs::set_permissions(path, perms);
        }
    }
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("attrib")
            .args(["+H", &path.to_string_lossy()])
            .output();
    }
}
