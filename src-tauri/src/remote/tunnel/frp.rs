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
        target: "se_manager::remote::tunnel",
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
        target: "se_manager::remote::tunnel",
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

/// T-H17 — the frpc `[[proxies]]` registration name.
///
/// # Why this parses instead of substring-matching
///
/// The proxy name is a wire identity: it is what the operator's `frps` logs,
/// what appears in `frps` dashboards, and what a `proxyName`-scoped server-side
/// rule matches on. It has zero coverage today — the sibling `tests` module
/// above asserts `serverAddr`, `localPort`, `type`, `customDomains`,
/// `remotePort` and the token escaping, but never `name`.
///
/// The obvious "fix" — `assert!(toml.contains("name = \"termul\""))` — would be
/// worthless: the literal in the assertion is a copy of the literal in
/// `render_frpc_toml`, so one `sed s/termul/se-manager/g` rewrites both and the
/// test stays green while every operator's `frps`-side proxy rule stops
/// matching. So this parses the *generated document*, extracts
/// `proxies[0].name` positionally, and compares it against the brand seam with
/// the post-rename value injected on this thread. Nothing brand-shaped is
/// inlined in the assertion.
///
/// A separate module from `tests` above so the existing coverage is untouched.
///
/// # Seam Wave 4 must add
///
/// `render_frpc_toml` must interpolate `crate::brand::canonical().frp_proxy_name`
/// rather than the hardcoded `name = "termul"` at `frp.rs:109`.
#[cfg(test)]
mod brand_parity_tests {
    use super::*;
    use crate::brand::{self, BrandCanonical};

    fn post_rename() -> BrandCanonical {
        BrandCanonical {
            frp_proxy_name: "se-manager",
            ..brand::DEFAULT_CANONICAL
        }
    }

    /// Unescapes one TOML basic string starting at the opening quote.
    /// Returns `None` when the value is not a quoted string.
    fn parse_basic_string(raw: &str) -> Option<String> {
        let mut chars = raw.trim().chars();
        if chars.next()? != '"' {
            return None;
        }
        let mut out = String::new();
        while let Some(ch) = chars.next() {
            match ch {
                '"' => return Some(out),
                '\\' => match chars.next()? {
                    'n' => out.push('\n'),
                    't' => out.push('\t'),
                    other => out.push(other),
                },
                other => out.push(other),
            }
        }
        None
    }

    /// Every `[[proxies]]` table in document order, as key -> string value.
    fn parse_proxies(document: &str) -> Vec<Vec<(String, String)>> {
        let mut proxies: Vec<Vec<(String, String)>> = Vec::new();
        let mut inside_proxy = false;
        for line in document.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if line == "[[proxies]]" {
                proxies.push(Vec::new());
                inside_proxy = true;
                continue;
            }
            if line.starts_with('[') {
                inside_proxy = false;
                continue;
            }
            if !inside_proxy {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            if let Some(parsed) = parse_basic_string(value) {
                proxies
                    .last_mut()
                    .expect("a [[proxies]] header preceded this key")
                    .push((key.trim().to_string(), parsed));
            }
        }
        proxies
    }

    fn tcp_config() -> TunnelConfig {
        TunnelConfig {
            provider: TunnelProviderKind::Frp,
            frp_server_addr: Some("1.2.3.4".to_string()),
            frp_server_port: Some(7000),
            frp_remote_port: Some(8443),
            ..TunnelConfig::default()
        }
    }

    /// The parser is load-bearing, so prove it reads a real generated document
    /// rather than always returning nothing. Not a `should_panic` ledger entry.
    #[test]
    fn proxy_parser_reads_the_generated_document() {
        // A token containing an escaped quote lives in `[auth]`, immediately
        // before `[[proxies]]` — the parser must not leak it into the proxy.
        let toml = render_frpc_toml(9000, &tcp_config(), "s3cret\"value").unwrap();
        let proxies = parse_proxies(&toml);
        assert_eq!(proxies.len(), 1, "generated config has exactly one proxy");
        let keys: Vec<&str> = proxies[0].iter().map(|(k, _)| k.as_str()).collect();
        assert!(keys.contains(&"name"), "proxies[0] carries a name: {keys:?}");
        assert!(
            !keys.contains(&"token"),
            "the [auth] token must not be parsed into the proxy table: {keys:?}"
        );
    }

    #[test]
    #[should_panic(expected = "must be crate::brand::canonical().frp_proxy_name")]
    fn frpc_proxy_name_comes_from_the_brand_seam() {
        let _guard = brand::override_canonical(post_rename());
        assert_ne!(
            brand::canonical().frp_proxy_name,
            brand::LEGACY.frp_proxy_name,
            "the post-rename injection did not take"
        );

        let toml = render_frpc_toml(9000, &tcp_config(), "tok").unwrap();
        let proxies = parse_proxies(&toml);
        let first = proxies
            .first()
            .unwrap_or_else(|| panic!("generated frpc config declares no [[proxies]]:\n{toml}"));
        let name = first
            .iter()
            .find(|(key, _)| key == "name")
            .map(|(_, value)| value.clone())
            .unwrap_or_else(|| panic!("generated [[proxies]] carries no name:\n{toml}"));

        assert_eq!(
            name,
            brand::canonical().frp_proxy_name,
            "the generated frpc proxies[0].name must be crate::brand::canonical().frp_proxy_name; \
             render_frpc_toml still emits the hardcoded {:?}",
            brand::LEGACY.frp_proxy_name,
        );
    }

    /// The same document with no override: the emitted name must equal the
    /// *shipped* canonical value. This is the evidence that the red above is a
    /// missing capability and not a broken parser or a bad injection — and it
    /// keeps working after Wave 5 flips `DEFAULT_CANONICAL`, at which point it
    /// becomes the check that the flip actually reached the generated document.
    #[test]
    fn frpc_proxy_name_matches_the_shipped_canonical_value() {
        let toml = render_frpc_toml(9000, &tcp_config(), "tok").unwrap();
        let proxies = parse_proxies(&toml);
        let name = proxies[0]
            .iter()
            .find(|(key, _)| key == "name")
            .map(|(_, value)| value.as_str())
            .expect("proxies[0].name");
        assert_eq!(name, brand::DEFAULT_CANONICAL.frp_proxy_name);
    }
}
