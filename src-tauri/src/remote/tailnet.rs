//! Tailnet (Tailscale / Headscale) address discovery for shared-live publish.
//!
//! Both the official Tailscale control plane and a self-hosted Headscale
//! server hand every node a stable IPv4 in the CGNAT range `100.64.0.0/10`
//! and run the same `tailscaled` client, so one discovery path serves both:
//!
//! 1. `tailscale status --json --peers=false` — authoritative. Yields the
//!    backend state, the node's tailnet IPv4, its MagicDNS name, and the
//!    MagicDNS suffix (`*.ts.net` for the official service, anything else for
//!    Headscale).
//! 2. Interface fallback — when the CLI is missing or unusable, a CGNAT
//!    address on a tunnel adapter (`utun*`, `tailscale*`, Windows
//!    `Tailscale`). Never a physical adapter: an ISP can number a real NIC
//!    from the same range (see `lan.rs`).
//!
//! WireGuard already encrypts tailnet traffic end to end, so publishing a
//! cleartext `http://100.x.y.z:port` Origin carries the same exposure as the
//! LAN mode: the bearer/pairing code remains the access secret.

use std::net::{IpAddr, Ipv4Addr};
use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// How long one `tailscale status` call may take before discovery gives up
/// on the CLI and falls back to interface enumeration.
const CLI_TIMEOUT: Duration = Duration::from_secs(4);

/// Which control plane the node is enrolled in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TailnetFlavor {
    /// Tailscale's hosted coordination server (MagicDNS suffix `*.ts.net`).
    Official,
    /// A self-hosted coordination server such as Headscale.
    SelfHosted,
    /// Found by interface enumeration only; the control plane is unknown.
    Unknown,
}

impl TailnetFlavor {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Official => "official",
            Self::SelfHosted => "selfHosted",
            Self::Unknown => "unknown",
        }
    }
}

/// This node's tailnet identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TailnetInfo {
    pub ipv4: Ipv4Addr,
    /// MagicDNS FQDN without the trailing dot, when MagicDNS is enabled.
    pub dns_name: Option<String>,
    pub flavor: TailnetFlavor,
}

/// Why no tailnet address is available. Codes are stable for the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TailnetError {
    /// No `tailscale` CLI and no tailnet adapter.
    NotFound,
    /// The client is installed but not connected (`Stopped`, `NeedsLogin`, …).
    NotRunning(String),
    /// Connected, but the node has no IPv4 tailnet address.
    NoAddress,
}

impl TailnetError {
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::NotFound => "TAILNET_NOT_FOUND",
            Self::NotRunning(_) => "TAILNET_NOT_RUNNING",
            Self::NoAddress => "TAILNET_NO_ADDRESS",
        }
    }

    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Self::NotFound => {
                "Tailscale is not installed or not connected on this computer".to_string()
            }
            Self::NotRunning(state) => {
                format!("Tailscale is not connected (state: {state}); sign in and connect first")
            }
            Self::NoAddress => "Tailscale is connected but has no IPv4 tailnet address".to_string(),
        }
    }
}

/// `100.64.0.0/10` — the range Tailscale and Headscale number nodes from.
#[must_use]
pub fn is_tailnet_ipv4(ip: Ipv4Addr) -> bool {
    let octets = ip.octets();
    octets[0] == 100 && (64..=127).contains(&octets[1])
}

#[must_use]
pub fn http_origin(host: &str, port: u16) -> String {
    format!("http://{host}:{port}")
}

/// Discover this node's tailnet identity. Blocking: runs the CLI with a
/// bounded timeout. Call from `spawn_blocking` on async paths.
pub fn discover_tailnet() -> Result<TailnetInfo, TailnetError> {
    let cli_result = run_status_cli();
    match cli_result {
        Some(Ok(bytes)) => match parse_status_json(&bytes) {
            Ok(info) => {
                log::info!(
                    target: "se_manager::remote::tailnet",
                    "operation=tailnet_discover source=cli flavor={} magic_dns={} stable_code=OK",
                    info.flavor.as_str(),
                    info.dns_name.is_some()
                );
                return Ok(info);
            }
            Err(error @ TailnetError::NotRunning(_)) => {
                // An installed, logged-out client is authoritative: a stale
                // adapter address would publish an Origin nobody can reach.
                log::warn!(
                    target: "se_manager::remote::tailnet",
                    "operation=tailnet_discover source=cli stable_code={}",
                    error.code()
                );
                return Err(error);
            }
            Err(error) => {
                log::warn!(
                    target: "se_manager::remote::tailnet",
                    "operation=tailnet_discover source=cli stable_code={} fallback=interfaces",
                    error.code()
                );
            }
        },
        Some(Err(())) => {
            log::warn!(
                target: "se_manager::remote::tailnet",
                "operation=tailnet_discover source=cli stable_code=TAILNET_CLI_FAILED fallback=interfaces"
            );
        }
        None => {}
    }
    let addrs = if_addrs::get_if_addrs().map_err(|_| TailnetError::NotFound)?;
    let ip = select_tailnet_ipv4(addrs.into_iter().map(|addr| {
        let ip = addr.ip();
        (addr.name, ip)
    }))
    .ok_or(TailnetError::NotFound)?;
    log::info!(
        target: "se_manager::remote::tailnet",
        "operation=tailnet_discover source=interfaces stable_code=OK"
    );
    Ok(TailnetInfo {
        ipv4: ip,
        dns_name: None,
        flavor: TailnetFlavor::Unknown,
    })
}

/// Async wrapper around [`discover_tailnet`].
pub async fn discover_tailnet_async() -> Result<TailnetInfo, TailnetError> {
    tokio::task::spawn_blocking(discover_tailnet)
        .await
        .unwrap_or(Err(TailnetError::NotFound))
}

/// Retry discovery until `deadline` elapses. At login the desktop app often
/// starts before `tailscaled` has connected; a restored session should wait
/// for it briefly instead of failing at once.
pub async fn discover_tailnet_with_grace(grace: Duration) -> Result<TailnetInfo, TailnetError> {
    let started = Instant::now();
    loop {
        let result = discover_tailnet_async().await;
        match result {
            Ok(info) => return Ok(info),
            Err(error) if started.elapsed() >= grace => return Err(error),
            Err(_) => tokio::time::sleep(Duration::from_secs(2)).await,
        }
    }
}

/// Candidate CLI locations. A GUI-launched app does not inherit the login
/// shell's PATH, so the common install paths are probed explicitly.
fn cli_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(value) = std::env::var("SE_TAILSCALE_PATH") {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            candidates.push(PathBuf::from(trimmed));
        }
    }
    #[cfg(target_os = "macos")]
    {
        candidates.push(PathBuf::from(
            "/Applications/Tailscale.app/Contents/MacOS/Tailscale",
        ));
        candidates.push(PathBuf::from("/opt/homebrew/bin/tailscale"));
        candidates.push(PathBuf::from("/usr/local/bin/tailscale"));
    }
    #[cfg(target_os = "linux")]
    {
        candidates.push(PathBuf::from("/usr/bin/tailscale"));
        candidates.push(PathBuf::from("/usr/local/bin/tailscale"));
        candidates.push(PathBuf::from("/usr/sbin/tailscale"));
        candidates.push(PathBuf::from("/snap/bin/tailscale"));
    }
    #[cfg(target_os = "windows")]
    {
        if let Ok(program_files) = std::env::var("ProgramFiles") {
            candidates.push(
                PathBuf::from(program_files)
                    .join("Tailscale")
                    .join("tailscale.exe"),
            );
        }
    }
    candidates
}

/// `Some(Ok(stdout))` when a CLI answered, `Some(Err(()))` when one was
/// found but failed, `None` when no CLI is installed.
fn run_status_cli() -> Option<Result<Vec<u8>, ()>> {
    let mut found_any = false;
    let mut candidates: Vec<PathBuf> = cli_candidates()
        .into_iter()
        .filter(|path| path.is_file())
        .collect();
    // Bare name last: resolves through PATH when it is inherited (Linux,
    // terminal-launched builds).
    candidates.push(PathBuf::from(if cfg!(windows) {
        "tailscale.exe"
    } else {
        "tailscale"
    }));
    for path in candidates {
        let mut command = std::process::Command::new(&path);
        command
            .args(["status", "--json", "--peers=false"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        configure_no_window(&mut command);
        let Ok(child) = command.spawn() else {
            continue;
        };
        found_any = true;
        match wait_with_timeout(child, CLI_TIMEOUT) {
            // `tailscale status` exits non-zero when stopped/logged out but
            // still prints the JSON document; parse whatever arrived.
            Some(stdout) if !stdout.is_empty() => return Some(Ok(stdout)),
            _ => continue,
        }
    }
    found_any.then_some(Err(()))
}

fn wait_with_timeout(mut child: std::process::Child, timeout: Duration) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut buffer = Vec::new();
        let _ = stdout.read_to_end(&mut buffer);
        buffer
    });
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(25));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    reader.join().ok()
}

#[cfg(target_os = "windows")]
fn configure_no_window(command: &mut std::process::Command) {
    use std::os::windows::process::CommandExt;
    command.creation_flags(0x0800_0000);
}
#[cfg(not(target_os = "windows"))]
fn configure_no_window(_command: &mut std::process::Command) {}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct StatusDoc {
    #[serde(default)]
    backend_state: String,
    #[serde(default, rename = "MagicDNSSuffix")]
    magic_dns_suffix: Option<String>,
    #[serde(default)]
    current_tailnet: Option<CurrentTailnet>,
    #[serde(default, rename = "Self")]
    self_node: Option<SelfNode>,
    #[serde(default, rename = "TailscaleIPs")]
    tailscale_ips: Option<Vec<String>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct CurrentTailnet {
    #[serde(default, rename = "MagicDNSSuffix")]
    magic_dns_suffix: Option<String>,
    #[serde(default, rename = "MagicDNSEnabled")]
    magic_dns_enabled: Option<bool>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct SelfNode {
    #[serde(default, rename = "DNSName")]
    dns_name: Option<String>,
    #[serde(default, rename = "TailscaleIPs")]
    tailscale_ips: Option<Vec<String>>,
}

/// Parse `tailscale status --json`. Pure so both control planes are
/// covered by fixture tests.
pub fn parse_status_json(bytes: &[u8]) -> Result<TailnetInfo, TailnetError> {
    let doc: StatusDoc = serde_json::from_slice(bytes).map_err(|_| TailnetError::NotFound)?;
    if doc.backend_state != "Running" {
        let state = if doc.backend_state.is_empty() {
            "Unknown".to_string()
        } else {
            doc.backend_state
        };
        return Err(TailnetError::NotRunning(state));
    }
    let ips = doc
        .self_node
        .as_ref()
        .and_then(|node| node.tailscale_ips.clone())
        .or(doc.tailscale_ips)
        .unwrap_or_default();
    let ipv4 = ips
        .iter()
        .filter_map(|raw| raw.parse::<Ipv4Addr>().ok())
        .find(|ip| is_tailnet_ipv4(*ip))
        .ok_or(TailnetError::NoAddress)?;
    let suffix = doc
        .current_tailnet
        .as_ref()
        .and_then(|tailnet| tailnet.magic_dns_suffix.clone())
        .or(doc.magic_dns_suffix)
        .map(|suffix| suffix.trim_end_matches('.').to_ascii_lowercase())
        .filter(|suffix| !suffix.is_empty());
    let flavor = match suffix.as_deref() {
        Some(suffix) if suffix == "ts.net" || suffix.ends_with(".ts.net") => {
            TailnetFlavor::Official
        }
        _ => TailnetFlavor::SelfHosted,
    };
    let magic_dns_enabled = doc
        .current_tailnet
        .as_ref()
        .and_then(|tailnet| tailnet.magic_dns_enabled)
        .unwrap_or(true);
    let dns_name = doc
        .self_node
        .and_then(|node| node.dns_name)
        .map(|name| name.trim_end_matches('.').to_ascii_lowercase())
        .filter(|name| magic_dns_enabled && is_dns_hostname(name));
    Ok(TailnetInfo {
        ipv4,
        dns_name,
        flavor,
    })
}

/// A MagicDNS FQDN usable as a URL host: ASCII labels, at least one dot.
fn is_dns_hostname(name: &str) -> bool {
    name.contains('.')
        && name.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
}

/// Interface fallback: a tailnet address on a tunnel adapter only.
fn select_tailnet_ipv4<I>(candidates: I) -> Option<Ipv4Addr>
where
    I: Iterator<Item = (String, IpAddr)>,
{
    candidates
        .filter(|(name, _)| is_tunnel_adapter(name))
        .find_map(|(_, ip)| match ip {
            IpAddr::V4(v4) if is_tailnet_ipv4(v4) => Some(v4),
            _ => None,
        })
}

fn is_tunnel_adapter(name: &str) -> bool {
    let lowered = name.to_ascii_lowercase();
    lowered.starts_with("utun") || lowered.starts_with("tailscale")
}

#[cfg(test)]
mod tests {
    use super::*;

    const OFFICIAL: &str = r#"{
        "Version": "1.80.0",
        "BackendState": "Running",
        "TailscaleIPs": ["100.101.102.103", "fd7a:115c:a1e0::1"],
        "Self": {
            "DNSName": "mac.tail1234.ts.net.",
            "TailscaleIPs": ["100.101.102.103", "fd7a:115c:a1e0::1"]
        },
        "MagicDNSSuffix": "tail1234.ts.net",
        "CurrentTailnet": {"Name": "me@example.com", "MagicDNSSuffix": "tail1234.ts.net", "MagicDNSEnabled": true}
    }"#;

    const HEADSCALE: &str = r#"{
        "BackendState": "Running",
        "Self": {"DNSName": "qsmacbook-pro.vpn.internal.", "TailscaleIPs": ["100.64.0.4"]},
        "MagicDNSSuffix": "vpn.internal",
        "CurrentTailnet": {"Name": "vpn.example.codes", "MagicDNSSuffix": "vpn.internal", "MagicDNSEnabled": true}
    }"#;

    #[test]
    fn official_tailscale_status_is_official_with_magic_dns() {
        let info = parse_status_json(OFFICIAL.as_bytes()).unwrap();
        assert_eq!(info.ipv4, Ipv4Addr::new(100, 101, 102, 103));
        assert_eq!(info.flavor, TailnetFlavor::Official);
        assert_eq!(info.dns_name.as_deref(), Some("mac.tail1234.ts.net"));
    }

    #[test]
    fn headscale_status_is_self_hosted() {
        let info = parse_status_json(HEADSCALE.as_bytes()).unwrap();
        assert_eq!(info.ipv4, Ipv4Addr::new(100, 64, 0, 4));
        assert_eq!(info.flavor, TailnetFlavor::SelfHosted);
        assert_eq!(info.dns_name.as_deref(), Some("qsmacbook-pro.vpn.internal"));
    }

    #[test]
    fn magic_dns_disabled_publishes_no_name() {
        let doc = r#"{"BackendState":"Running","Self":{"DNSName":"n.vpn.internal.","TailscaleIPs":["100.64.0.9"]},
            "CurrentTailnet":{"MagicDNSSuffix":"vpn.internal","MagicDNSEnabled":false}}"#;
        let info = parse_status_json(doc.as_bytes()).unwrap();
        assert_eq!(info.dns_name, None);
        assert_eq!(info.flavor, TailnetFlavor::SelfHosted);
    }

    #[test]
    fn stopped_or_logged_out_client_is_not_running() {
        for state in ["Stopped", "NeedsLogin", "Starting"] {
            let doc =
                format!(r#"{{"BackendState":"{state}","Self":{{"TailscaleIPs":["100.64.0.4"]}}}}"#);
            assert_eq!(
                parse_status_json(doc.as_bytes()),
                Err(TailnetError::NotRunning(state.to_string()))
            );
        }
    }

    #[test]
    fn running_without_ipv4_reports_no_address() {
        let doc = r#"{"BackendState":"Running","Self":{"TailscaleIPs":["fd7a:115c:a1e0::1"]}}"#;
        assert_eq!(
            parse_status_json(doc.as_bytes()),
            Err(TailnetError::NoAddress)
        );
    }

    #[test]
    fn garbage_is_not_found() {
        assert_eq!(parse_status_json(b"not json"), Err(TailnetError::NotFound));
    }

    #[test]
    fn tailnet_range_is_cgnat() {
        assert!(is_tailnet_ipv4(Ipv4Addr::new(100, 64, 0, 1)));
        assert!(is_tailnet_ipv4(Ipv4Addr::new(100, 127, 255, 254)));
        assert!(!is_tailnet_ipv4(Ipv4Addr::new(100, 128, 0, 1)));
        assert!(!is_tailnet_ipv4(Ipv4Addr::new(192, 168, 1, 1)));
    }

    #[test]
    fn interface_fallback_ignores_physical_cgnat() {
        let candidates = vec![
            ("en0".to_string(), IpAddr::V4(Ipv4Addr::new(100, 70, 1, 2))),
            (
                "utun3".to_string(),
                IpAddr::V4(Ipv4Addr::new(198, 18, 0, 1)),
            ),
            (
                "utun15".to_string(),
                IpAddr::V4(Ipv4Addr::new(100, 64, 0, 4)),
            ),
        ];
        assert_eq!(
            select_tailnet_ipv4(candidates.into_iter()),
            Some(Ipv4Addr::new(100, 64, 0, 4))
        );
    }

    #[test]
    fn interface_fallback_accepts_linux_and_windows_names() {
        let linux = vec![(
            "tailscale0".to_string(),
            IpAddr::V4(Ipv4Addr::new(100, 90, 0, 1)),
        )];
        assert!(select_tailnet_ipv4(linux.into_iter()).is_some());
        let windows = vec![(
            "Tailscale".to_string(),
            IpAddr::V4(Ipv4Addr::new(100, 90, 0, 2)),
        )];
        assert!(select_tailnet_ipv4(windows.into_iter()).is_some());
        let none = vec![("en0".to_string(), IpAddr::V4(Ipv4Addr::new(100, 90, 0, 3)))];
        assert_eq!(select_tailnet_ipv4(none.into_iter()), None);
    }

    #[test]
    fn dns_hostname_validation() {
        assert!(is_dns_hostname("mac.tail1234.ts.net"));
        assert!(!is_dns_hostname("qs的macbook"));
        assert!(!is_dns_hostname("single"));
        assert!(!is_dns_hostname("a..b"));
    }
}
