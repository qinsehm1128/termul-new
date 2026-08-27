//! Durable tunnel settings. Non-secrets live in `config.json`; tokens saved
//! from Preferences → Remote Access live in `secrets.json` next to it and are
//! cached in memory after the first read. The renderer only sees `*TokenSet`
//! booleans.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use url::Url;

use crate::acp::atomic_file;

/// Default loopback port for named Cloudflare tunnels so the operator can
/// point a remotely-managed ingress at a stable `http://127.0.0.1:18787`.
pub const DEFAULT_NAMED_LOCAL_PORT: u16 = 18787;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum TunnelProviderKind {
    #[default]
    CloudflareQuick,
    CloudflareNamed,
    Frp,
    SshReverse,
}

impl TunnelProviderKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CloudflareQuick => "cloudflareQuick",
            Self::CloudflareNamed => "cloudflareNamed",
            Self::Frp => "frp",
            Self::SshReverse => "sshReverse",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TunnelConfig {
    pub provider: TunnelProviderKind,
    /// Hostname only (`termul.example.com`), never a URL with credentials.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cloudflare_named_hostname: Option<String>,
    /// Loopback port the named-tunnel ingress should target. Ignored by Quick.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cloudflare_named_local_port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frp_server_addr: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frp_server_port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frp_custom_domain: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frp_remote_port: Option<u16>,
    /// When true the QR uses `https://`; FRP itself may still speak HTTP to origin.
    #[serde(default = "default_true")]
    pub frp_public_https: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh_host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh_port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh_user: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh_remote_port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh_public_hostname: Option<String>,
    #[serde(default = "default_true")]
    pub ssh_public_https: bool,
}

fn default_true() -> bool {
    true
}

impl Default for TunnelConfig {
    fn default() -> Self {
        Self {
            provider: TunnelProviderKind::CloudflareQuick,
            cloudflare_named_hostname: None,
            cloudflare_named_local_port: None,
            frp_server_addr: None,
            frp_server_port: None,
            frp_custom_domain: None,
            frp_remote_port: None,
            frp_public_https: true,
            ssh_host: None,
            ssh_port: None,
            ssh_user: None,
            ssh_remote_port: None,
            ssh_public_hostname: None,
            ssh_public_https: true,
        }
    }
}

impl TunnelConfig {
    /// Named Cloudflare needs a stable local port; other providers keep OS-assigned `0`.
    #[must_use]
    pub fn preferred_bind_port(&self) -> Option<u16> {
        match self.provider {
            TunnelProviderKind::CloudflareNamed => Some(
                self.cloudflare_named_local_port
                    .unwrap_or(DEFAULT_NAMED_LOCAL_PORT),
            ),
            TunnelProviderKind::CloudflareQuick
            | TunnelProviderKind::Frp
            | TunnelProviderKind::SshReverse => None,
        }
    }

    pub fn validate_for_start(&self) -> Result<(), String> {
        match self.provider {
            TunnelProviderKind::CloudflareQuick => Ok(()),
            TunnelProviderKind::CloudflareNamed => {
                normalize_hostname(self.cloudflare_named_hostname.as_deref().unwrap_or(""))?;
                if let Some(port) = self.cloudflare_named_local_port {
                    if port == 0 {
                        return Err("named Cloudflare local port must be 1..=65535".to_string());
                    }
                }
                Ok(())
            }
            TunnelProviderKind::Frp => {
                let addr = self
                    .frp_server_addr
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| "FRP server address is required".to_string())?;
                if addr.contains('/') || addr.contains(' ') {
                    return Err("FRP server address must be a host or IP, not a URL".to_string());
                }
                if self.frp_server_port == Some(0) {
                    return Err("FRP server port must be 1..=65535".to_string());
                }
                let domain = self
                    .frp_custom_domain
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty());
                if let Some(domain) = domain {
                    normalize_hostname(domain)?;
                }
                if domain.is_none() && self.frp_remote_port.unwrap_or(0) == 0 {
                    return Err(
                        "FRP requires a custom domain or a remote port for the public URL"
                            .to_string(),
                    );
                }
                Ok(())
            }
            TunnelProviderKind::SshReverse => {
                let host = self
                    .ssh_host
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| "SSH host is required".to_string())?;
                if host.contains('/') || host.contains(' ') {
                    return Err("SSH host must be a host or IP, not a URL".to_string());
                }
                if self.ssh_port == Some(0) {
                    return Err("SSH port must be 1..=65535".to_string());
                }
                let user = self
                    .ssh_user
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| "SSH user is required".to_string())?;
                if user.contains(' ') || user.contains('@') {
                    return Err("SSH user must be a login name".to_string());
                }
                if self.ssh_remote_port.unwrap_or(0) == 0 {
                    return Err("SSH remote port is required".to_string());
                }
                normalize_public_http_origin(
                    self.ssh_public_hostname.as_deref().unwrap_or(""),
                    self.ssh_public_https,
                )?;
                Ok(())
            }
        }
    }

    pub fn public_origin(&self) -> Result<String, String> {
        match self.provider {
            TunnelProviderKind::CloudflareQuick => {
                Err("quick tunnel Origin is produced by cloudflared".to_string())
            }
            TunnelProviderKind::CloudflareNamed => {
                let host =
                    normalize_hostname(self.cloudflare_named_hostname.as_deref().unwrap_or(""))?;
                Ok(format!("https://{host}"))
            }
            TunnelProviderKind::Frp => {
                let scheme = if self.frp_public_https {
                    "https"
                } else {
                    "http"
                };
                if let Some(domain) = self
                    .frp_custom_domain
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                {
                    let host = normalize_hostname(domain)?;
                    return Ok(format!("{scheme}://{host}"));
                }
                let addr = self
                    .frp_server_addr
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| "FRP server address is required".to_string())?;
                let port = self.frp_remote_port.filter(|p| *p > 0).ok_or_else(|| {
                    "FRP remote port is required without a custom domain".to_string()
                })?;
                Ok(format!("{scheme}://{addr}:{port}"))
            }
            TunnelProviderKind::SshReverse => normalize_public_http_origin(
                self.ssh_public_hostname.as_deref().unwrap_or(""),
                self.ssh_public_https,
            ),
        }
    }
}

/// Renderer-facing view: never includes raw secrets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TunnelConfigView {
    pub provider: TunnelProviderKind,
    pub cloudflare_named_hostname: Option<String>,
    pub cloudflare_named_local_port: Option<u16>,
    pub cloudflare_named_token_set: bool,
    pub frp_server_addr: Option<String>,
    pub frp_server_port: Option<u16>,
    pub frp_custom_domain: Option<String>,
    pub frp_remote_port: Option<u16>,
    pub frp_public_https: bool,
    pub frp_token_set: bool,
    pub ssh_host: Option<String>,
    pub ssh_port: Option<u16>,
    pub ssh_user: Option<String>,
    pub ssh_remote_port: Option<u16>,
    pub ssh_public_hostname: Option<String>,
    pub ssh_public_https: bool,
    pub ssh_private_key_set: bool,
}

/// Partial update from the renderer. `None` on a secret field means "leave as-is";
/// `Some("")` clears the stored secret; any other `Some` replaces it.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TunnelConfigUpdate {
    pub provider: TunnelProviderKind,
    #[serde(default)]
    pub cloudflare_named_hostname: Option<String>,
    #[serde(default)]
    pub cloudflare_named_local_port: Option<u16>,
    #[serde(default)]
    pub cloudflare_named_token: Option<String>,
    #[serde(default)]
    pub frp_server_addr: Option<String>,
    #[serde(default)]
    pub frp_server_port: Option<u16>,
    #[serde(default)]
    pub frp_custom_domain: Option<String>,
    #[serde(default)]
    pub frp_remote_port: Option<u16>,
    #[serde(default)]
    pub frp_public_https: Option<bool>,
    #[serde(default)]
    pub frp_token: Option<String>,
    #[serde(default)]
    pub ssh_host: Option<String>,
    #[serde(default)]
    pub ssh_port: Option<u16>,
    #[serde(default)]
    pub ssh_user: Option<String>,
    #[serde(default)]
    pub ssh_remote_port: Option<u16>,
    #[serde(default)]
    pub ssh_public_hostname: Option<String>,
    #[serde(default)]
    pub ssh_public_https: Option<bool>,
    #[serde(default)]
    pub ssh_private_key: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct TunnelSecrets {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cloudflare_named_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    frp_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ssh_private_key: Option<String>,
    /// Desktop pairing bearer. Reused while remote access stays wanted.
    /// Never returned to the renderer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pairing_token: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TunnelConfigStore {
    path: PathBuf,
    secrets_path: PathBuf,
    secrets: Arc<Mutex<Option<TunnelSecrets>>>,
}

impl TunnelConfigStore {
    #[must_use]
    pub fn new(app_data_dir: PathBuf) -> Self {
        let dir = app_data_dir.join("remote-tunnel");
        Self {
            path: dir.join("config.json"),
            secrets_path: dir.join("secrets.json"),
            secrets: Arc::new(Mutex::new(None)),
        }
    }

    #[cfg(test)]
    #[must_use]
    pub fn for_path(path: PathBuf) -> Self {
        let secrets_path = path
            .parent()
            .map(|parent| parent.join("secrets.json"))
            .unwrap_or_else(|| PathBuf::from("secrets.json"));
        Self {
            path,
            secrets_path,
            secrets: Arc::new(Mutex::new(None)),
        }
    }

    #[must_use]
    pub(crate) fn parent_dir(&self) -> PathBuf {
        self.path
            .parent()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."))
    }

    pub fn load(&self) -> Result<TunnelConfig, String> {
        if !self.path.exists() {
            return Ok(TunnelConfig::default());
        }
        let bytes =
            std::fs::read(&self.path).map_err(|e| format!("failed to read tunnel config: {e}"))?;
        serde_json::from_slice(&bytes).map_err(|e| format!("tunnel config is invalid: {e}"))
    }

    pub fn save(&self, config: &TunnelConfig) -> Result<(), String> {
        // Named/FRP may be saved incomplete while the operator fills fields.
        // `validate_for_start` is the hard gate at tunnel start.
        let bytes = serde_json::to_vec_pretty(config)
            .map_err(|e| format!("failed to serialize tunnel config: {e}"))?;
        atomic_file::replace(&self.path, &bytes)
            .map_err(|e| format!("failed to write tunnel config: {e}"))
    }

    pub fn apply_update(&self, update: TunnelConfigUpdate) -> Result<TunnelConfig, String> {
        let mut config = self.load()?;
        config.provider = update.provider;
        config.cloudflare_named_hostname = empty_to_none(update.cloudflare_named_hostname);
        config.cloudflare_named_local_port = update.cloudflare_named_local_port.filter(|p| *p > 0);
        config.frp_server_addr = empty_to_none(update.frp_server_addr);
        config.frp_server_port = update.frp_server_port.filter(|p| *p > 0);
        config.frp_custom_domain = empty_to_none(update.frp_custom_domain);
        config.frp_remote_port = update.frp_remote_port.filter(|p| *p > 0);
        if let Some(https) = update.frp_public_https {
            config.frp_public_https = https;
        }
        config.ssh_host = empty_to_none(update.ssh_host);
        config.ssh_port = update.ssh_port.filter(|p| *p > 0);
        config.ssh_user = empty_to_none(update.ssh_user);
        config.ssh_remote_port = update.ssh_remote_port.filter(|p| *p > 0);
        config.ssh_public_hostname = empty_to_none(update.ssh_public_hostname);
        if let Some(https) = update.ssh_public_https {
            config.ssh_public_https = https;
        }
        self.apply_secret_update(
            update.cloudflare_named_token,
            update.frp_token,
            update.ssh_private_key,
        )?;
        self.save(&config)?;
        log::info!(
            target: "termul::remote::tunnel",
            "operation=tunnel_config_save provider={} stable_code=OK",
            config.provider.as_str()
        );
        Ok(config)
    }

    pub fn view(&self) -> Result<TunnelConfigView, String> {
        let config = self.load()?;
        let secrets = self.load_secrets()?;
        Ok(TunnelConfigView {
            provider: config.provider,
            cloudflare_named_hostname: config.cloudflare_named_hostname,
            cloudflare_named_local_port: config.cloudflare_named_local_port,
            cloudflare_named_token_set: token_is_set(secrets.cloudflare_named_token.as_deref()),
            frp_server_addr: config.frp_server_addr,
            frp_server_port: config.frp_server_port,
            frp_custom_domain: config.frp_custom_domain,
            frp_remote_port: config.frp_remote_port,
            frp_public_https: config.frp_public_https,
            frp_token_set: token_is_set(secrets.frp_token.as_deref()),
            ssh_host: config.ssh_host,
            ssh_port: config.ssh_port,
            ssh_user: config.ssh_user,
            ssh_remote_port: config.ssh_remote_port,
            ssh_public_hostname: config.ssh_public_hostname,
            ssh_public_https: config.ssh_public_https,
            ssh_private_key_set: token_is_set(secrets.ssh_private_key.as_deref()),
        })
    }

    pub fn named_token(&self) -> Result<String, String> {
        required_stored_secret(
            self.load_secrets()?.cloudflare_named_token,
            "Cloudflare named-tunnel token",
        )
    }

    pub fn require_frp_token(&self) -> Result<String, String> {
        required_stored_secret(self.load_secrets()?.frp_token, "FRP auth token")
    }

    pub fn ssh_private_key(&self) -> Result<Option<String>, String> {
        let secrets = self.load_secrets()?;
        Ok(empty_to_none(secrets.ssh_private_key))
    }

    pub fn pairing_token(&self) -> Result<Option<String>, String> {
        Ok(empty_to_none(self.load_secrets()?.pairing_token))
    }

    pub fn set_pairing_token(&self, token: Option<&str>) -> Result<(), String> {
        let mut secrets = self.load_secrets()?;
        secrets.pairing_token = empty_to_none(token.map(str::to_string));
        write_secrets(&self.secrets_path, &secrets)?;
        let mut guard = self
            .secrets
            .lock()
            .map_err(|_| "tunnel secret cache is unavailable".to_string())?;
        *guard = Some(secrets);
        Ok(())
    }

    fn load_secrets(&self) -> Result<TunnelSecrets, String> {
        let mut guard = self
            .secrets
            .lock()
            .map_err(|_| "tunnel secret cache is unavailable".to_string())?;
        if let Some(cached) = guard.clone() {
            return Ok(cached);
        }
        let loaded = if self.secrets_path.exists() {
            let bytes = std::fs::read(&self.secrets_path)
                .map_err(|error| format!("failed to read tunnel secrets: {error}"))?;
            serde_json::from_slice(&bytes)
                .map_err(|error| format!("tunnel secrets are invalid: {error}"))?
        } else {
            TunnelSecrets::default()
        };
        *guard = Some(loaded.clone());
        Ok(loaded)
    }

    fn apply_secret_update(
        &self,
        cloudflare_named_token: Option<String>,
        frp_token: Option<String>,
        ssh_private_key: Option<String>,
    ) -> Result<(), String> {
        if cloudflare_named_token.is_none() && frp_token.is_none() && ssh_private_key.is_none() {
            return Ok(());
        }
        let mut secrets = self.load_secrets()?;
        if let Some(value) = cloudflare_named_token {
            secrets.cloudflare_named_token = empty_to_none(Some(value));
        }
        if let Some(value) = frp_token {
            secrets.frp_token = empty_to_none(Some(value));
        }
        if let Some(value) = ssh_private_key {
            secrets.ssh_private_key = empty_to_none(Some(value));
        }
        write_secrets(&self.secrets_path, &secrets)?;
        let mut guard = self
            .secrets
            .lock()
            .map_err(|_| "tunnel secret cache is unavailable".to_string())?;
        *guard = Some(secrets);
        Ok(())
    }
}

fn empty_to_none(value: Option<String>) -> Option<String> {
    value.and_then(|s| {
        let trimmed = s.trim().to_string();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    })
}

fn token_is_set(value: Option<&str>) -> bool {
    value.is_some_and(|secret| !secret.trim().is_empty())
}

fn required_stored_secret(value: Option<String>, label: &str) -> Result<String, String> {
    value
        .and_then(|secret| {
            let trimmed = secret.trim().to_string();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed)
            }
        })
        .ok_or_else(|| format!("{label} is not set"))
}

fn write_secrets(path: &PathBuf, secrets: &TunnelSecrets) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(secrets)
        .map_err(|error| format!("failed to serialize tunnel secrets: {error}"))?;
    atomic_file::replace(path, &bytes)
        .map_err(|error| format!("failed to write tunnel secrets: {error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(path) {
            let mut perms = meta.permissions();
            perms.set_mode(0o600);
            let _ = std::fs::set_permissions(path, perms);
        }
    }
    Ok(())
}

/// Accept `example.com` or `https://example.com`; reject credentials and paths.
pub fn normalize_hostname(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return Err("public hostname is required".to_string());
    }
    let candidate = if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    };
    let parsed =
        Url::parse(&candidate).map_err(|_| "public hostname is not a valid host".to_string())?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("public hostname must use http or https".to_string());
    }
    if parsed.username() != "" || parsed.password().is_some() {
        return Err("public hostname must not include credentials".to_string());
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| "public hostname is not a valid host".to_string())?;
    if host.eq_ignore_ascii_case("localhost") || host == "127.0.0.1" || host == "::1" {
        return Err("public hostname must not be a loopback address".to_string());
    }
    let path = parsed.path();
    if !path.is_empty() && path != "/" {
        return Err("public hostname must not include a path".to_string());
    }
    Ok(host.to_string())
}

/// Accept `example.com`, `example.com:8443`, or a full http(s) URL.
pub fn normalize_public_http_origin(raw: &str, https: bool) -> Result<String, String> {
    let trimmed = raw.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return Err("public hostname is required".to_string());
    }
    let scheme = if https { "https" } else { "http" };
    let candidate = if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("{scheme}://{trimmed}")
    };
    let parsed =
        Url::parse(&candidate).map_err(|_| "public hostname is not a valid host".to_string())?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("public hostname must use http or https".to_string());
    }
    if parsed.username() != "" || parsed.password().is_some() {
        return Err("public hostname must not include credentials".to_string());
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| "public hostname is not a valid host".to_string())?;
    if host.eq_ignore_ascii_case("localhost") || host == "127.0.0.1" || host == "::1" {
        return Err("public hostname must not be a loopback address".to_string());
    }
    let path = parsed.path();
    if !path.is_empty() && path != "/" {
        return Err("public hostname must not include a path".to_string());
    }
    match parsed.port() {
        Some(port) => Ok(format!("{scheme}://{host}:{port}")),
        None => Ok(format!("{scheme}://{host}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_provider_is_quick_tunnel() {
        assert_eq!(
            TunnelConfig::default().provider,
            TunnelProviderKind::CloudflareQuick
        );
        assert_eq!(TunnelConfig::default().preferred_bind_port(), None);
    }

    #[test]
    fn named_provider_uses_stable_local_port() {
        let mut config = TunnelConfig::default();
        config.provider = TunnelProviderKind::CloudflareNamed;
        assert_eq!(config.preferred_bind_port(), Some(DEFAULT_NAMED_LOCAL_PORT));
        config.cloudflare_named_local_port = Some(8787);
        assert_eq!(config.preferred_bind_port(), Some(8787));
    }

    #[test]
    fn named_origin_normalizes_hostname() {
        let config = TunnelConfig {
            provider: TunnelProviderKind::CloudflareNamed,
            cloudflare_named_hostname: Some("https://Termul.Example.com/".to_string()),
            ..TunnelConfig::default()
        };
        let origin = config.public_origin().unwrap();
        assert!(origin.eq_ignore_ascii_case("https://Termul.Example.com"));
    }

    #[test]
    fn hostname_rejects_credentials_and_loopback() {
        assert!(normalize_hostname("https://user:pass@example.com").is_err());
        assert!(normalize_hostname("localhost").is_err());
        assert!(normalize_hostname("https://example.com/path").is_err());
        assert_eq!(normalize_hostname("example.com").unwrap(), "example.com");
    }

    #[test]
    fn frp_requires_domain_or_remote_port() {
        let mut config = TunnelConfig {
            provider: TunnelProviderKind::Frp,
            frp_server_addr: Some("1.2.3.4".to_string()),
            ..TunnelConfig::default()
        };
        assert!(config.validate_for_start().is_err());
        config.frp_remote_port = Some(8443);
        assert!(config.validate_for_start().is_ok());
        assert_eq!(config.public_origin().unwrap(), "https://1.2.3.4:8443");
        config.frp_public_https = false;
        config.frp_custom_domain = Some("termul.example.com".to_string());
        assert_eq!(config.public_origin().unwrap(), "http://termul.example.com");
    }

    #[test]
    fn ssh_reverse_requires_host_user_port_and_public_hostname() {
        let mut config = TunnelConfig {
            provider: TunnelProviderKind::SshReverse,
            ssh_host: Some("vps.example.com".to_string()),
            ssh_user: Some("termul".to_string()),
            ssh_remote_port: Some(18787),
            ssh_public_hostname: Some("remote.example.com".to_string()),
            ..TunnelConfig::default()
        };
        assert!(config.validate_for_start().is_ok());
        assert_eq!(
            config.public_origin().unwrap(),
            "https://remote.example.com"
        );
        config.ssh_public_https = false;
        config.ssh_public_hostname = Some("remote.example.com:8443".to_string());
        assert_eq!(
            config.public_origin().unwrap(),
            "http://remote.example.com:8443"
        );
        config.ssh_user = None;
        assert!(config.validate_for_start().is_err());
    }

    #[test]
    fn view_never_serializes_token_field_names_as_values() {
        let view = TunnelConfigView {
            provider: TunnelProviderKind::CloudflareQuick,
            cloudflare_named_hostname: None,
            cloudflare_named_local_port: None,
            cloudflare_named_token_set: false,
            frp_server_addr: None,
            frp_server_port: None,
            frp_custom_domain: None,
            frp_remote_port: None,
            frp_public_https: true,
            frp_token_set: false,
            ssh_host: None,
            ssh_port: None,
            ssh_user: None,
            ssh_remote_port: None,
            ssh_public_hostname: None,
            ssh_public_https: true,
            ssh_private_key_set: false,
        };
        let json = serde_json::to_string(&view).unwrap();
        assert!(!json.contains("eyJ"));
        assert!(json.contains("cloudflareNamedTokenSet"));
        assert!(!json.contains("cloudflareNamedToken\""));
    }

    #[test]
    fn persist_roundtrip_omits_none_fields() {
        let dir = tempfile::tempdir().unwrap();
        let store = TunnelConfigStore::for_path(dir.path().join("config.json"));
        let config = TunnelConfig::default();
        store.save(&config).unwrap();
        let loaded = store.load().unwrap();
        assert_eq!(loaded, config);
    }

    #[test]
    fn settings_tokens_roundtrip_without_keyring_probes() {
        let dir = tempfile::tempdir().unwrap();
        let store = TunnelConfigStore::for_path(dir.path().join("config.json"));
        store
            .apply_update(TunnelConfigUpdate {
                provider: TunnelProviderKind::CloudflareNamed,
                cloudflare_named_hostname: Some("termul.example.com".to_string()),
                cloudflare_named_local_port: Some(18787),
                cloudflare_named_token: Some("named-token".to_string()),
                frp_server_addr: None,
                frp_server_port: None,
                frp_custom_domain: None,
                frp_remote_port: None,
                frp_public_https: None,
                frp_token: None,
                ssh_host: None,
                ssh_port: None,
                ssh_user: None,
                ssh_remote_port: None,
                ssh_public_hostname: None,
                ssh_public_https: None,
                ssh_private_key: None,
            })
            .unwrap();
        let view = store.view().unwrap();
        assert!(view.cloudflare_named_token_set);
        assert!(!view.frp_token_set);
        assert_eq!(store.named_token().unwrap(), "named-token");
        let secrets = std::fs::read_to_string(dir.path().join("secrets.json")).unwrap();
        assert!(secrets.contains("named-token"));
    }

    #[test]
    fn pairing_token_roundtrips_in_secrets_without_keyring() {
        let dir = tempfile::tempdir().unwrap();
        let store = TunnelConfigStore::for_path(dir.path().join("config.json"));
        assert_eq!(store.pairing_token().unwrap(), None);
        store
            .set_pairing_token(Some("pairing-bearer-from-settings"))
            .unwrap();
        assert_eq!(
            store.pairing_token().unwrap().as_deref(),
            Some("pairing-bearer-from-settings")
        );
        let secrets = std::fs::read_to_string(dir.path().join("secrets.json")).unwrap();
        assert!(secrets.contains("pairing-bearer-from-settings"));
        store.set_pairing_token(None).unwrap();
        assert_eq!(store.pairing_token().unwrap(), None);
    }
}
