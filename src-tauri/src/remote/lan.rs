//! LAN address helpers for desktop shared-live publish URLs.

use std::net::{IpAddr, Ipv4Addr};

/// Best-effort IPv4 the phone can reach on the same Wi-Fi.
///
/// Interface enumeration with a name allow-list is the single source of
/// truth. A route probe follows the default route — which a proxy TUN
/// (Clash/Surge fake-ip) or a full-tunnel VPN (WireGuard 10/8, Tailscale
/// CGNAT) owns — so it can never be trusted to name a phone-reachable LAN
/// address; there is deliberately no route fallback. When no physical
/// adapter has a usable address, LAN mode publishes nothing.
#[must_use]
pub fn discover_lan_ipv4() -> Option<Ipv4Addr> {
    let addrs = if_addrs::get_if_addrs().ok()?;
    select_lan_ipv4(addrs.into_iter().map(|addr| {
        let ip = addr.ip();
        (addr.name, ip)
    }))
}

/// Pure selection over (interface name, address) candidates so the policy is
/// unit-testable without any real interfaces: physical adapters only, usable
/// ranges only, RFC1918 preferred.
fn select_lan_ipv4<I>(candidates: I) -> Option<Ipv4Addr>
where
    I: Iterator<Item = (String, IpAddr)>,
{
    let mut best: Option<(u8, Ipv4Addr)> = None;
    for (name, ip) in candidates {
        let IpAddr::V4(v4) = ip else { continue };
        if !is_physical_adapter(&name) || !is_usable_lan_v4(v4) {
            continue;
        }
        let rank = if v4.is_private() { 1 } else { 2 };
        if best.is_none_or(|(best_rank, _)| best_rank > rank) {
            best = Some((rank, v4));
        }
    }
    best.map(|(_, ip)| ip)
}

/// Physical adapters by name allow-list (mirrors the phone-side accept
/// policy in ios/SeRemote/Models/RemoteLink.swift — keep the two in step):
/// `en*` covers macOS en0… and Linux predictable names (eno/enp/ens/enx),
/// `eth*`/`em*` classic ethernet, `wlan*`/`wl*`/`wifi`/`wi-fi` wireless,
/// plus Windows friendly names. vEthernet (Hyper-V) is explicitly excluded.
/// Docker (docker0, veth, cni, br-), WireGuard (wg), Tailscale, ZeroTier,
/// utun/tap, awdl, and VM bridges never match an allow-list entry.
fn is_physical_adapter(name: &str) -> bool {
    let lowered = name.to_ascii_lowercase();
    if lowered.starts_with("vethernet") {
        return false;
    }
    lowered.starts_with("en")
        || lowered.starts_with("eth")
        || lowered.starts_with("em")
        || lowered.starts_with("wlan")
        || lowered.starts_with("wl")
        || lowered.starts_with("wifi")
        || lowered.starts_with("wi-fi")
        || lowered.starts_with("ethernet")
}

// No route-based fallback: see discover_lan_ipv4 — the default route belongs
// to whatever tunnel owns it, and a UDP probe cannot tell a physical LAN
// source from a VPN overlay source.

#[must_use]
pub fn is_usable_lan_v4(ip: Ipv4Addr) -> bool {
    !ip.is_unspecified()
        && !ip.is_loopback()
        && !ip.is_link_local()
        && !ip.is_multicast()
        && !ip.is_broadcast()
        // 198.18.0.0/15 is RFC 2544 benchmark space — and the fake-ip pool
        // proxy TUN adapters answer DNS with. Publishing it hands the phone
        // an address no physical network has.
        && !is_benchmark_range(ip)
        // RFC 5737 documentation ranges never appear on a real LAN.
        && !is_documentation_range(ip)
}

fn is_benchmark_range(ip: Ipv4Addr) -> bool {
    let octets = ip.octets();
    octets[0] == 198 && (octets[1] & 0xFE) == 18
}

fn is_documentation_range(ip: Ipv4Addr) -> bool {
    matches!(ip.octets(), [192, 0, 2, _] | [198, 51, 100, _] | [203, 0, 113, _])
}

#[must_use]
pub fn lan_http_origin(ip: Ipv4Addr, port: u16) -> String {
    format!("http://{ip}:{port}")
}

/// Attach `#access_token=` to an Origin. Returns `None` when the origin is not
/// an http(s) URL — callers must never publish a token-less fallback.
#[must_use]
pub fn credentialed_access_url(origin: &str, bearer: &str) -> Option<String> {
    if bearer.is_empty() {
        return None;
    }
    let mut url = url::Url::parse(origin).ok()?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return None;
    }
    url.set_fragment(Some(&format!("access_token={bearer}")));
    Some(url.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usable_lan_rejects_loopback_and_link_local() {
        assert!(is_usable_lan_v4(Ipv4Addr::new(192, 168, 1, 8)));
        assert!(is_usable_lan_v4(Ipv4Addr::new(10, 0, 0, 5)));
        assert!(!is_usable_lan_v4(Ipv4Addr::LOCALHOST));
        assert!(!is_usable_lan_v4(Ipv4Addr::UNSPECIFIED));
        assert!(!is_usable_lan_v4(Ipv4Addr::new(169, 254, 1, 1)));
    }

    #[test]
    fn usable_lan_rejects_benchmark_and_documentation_ranges() {
        // Fake-ip pool handed out by proxy TUN adapters.
        assert!(!is_usable_lan_v4(Ipv4Addr::new(198, 18, 0, 1)));
        assert!(!is_usable_lan_v4(Ipv4Addr::new(198, 19, 255, 9)));
        // RFC 5737 documentation ranges.
        assert!(!is_usable_lan_v4(Ipv4Addr::new(192, 0, 2, 7)));
        assert!(!is_usable_lan_v4(Ipv4Addr::new(203, 0, 113, 7)));
        // CGNAT on a physical adapter is a real, reachable LAN address.
        assert!(is_usable_lan_v4(Ipv4Addr::new(100, 64, 3, 4)));
    }

    #[test]
    fn selection_prefers_physical_rfc1918_over_proxy_tun() {
        let candidates = vec![
            ("utun4".to_string(), IpAddr::V4(Ipv4Addr::new(198, 18, 0, 1))),
            ("en0".to_string(), IpAddr::V4(Ipv4Addr::new(192, 168, 1, 8))),
            ("awdl0".to_string(), IpAddr::V4(Ipv4Addr::new(169, 254, 12, 1))),
        ];
        assert_eq!(
            select_lan_ipv4(candidates.into_iter()),
            Some(Ipv4Addr::new(192, 168, 1, 8))
        );
    }

    #[test]
    fn selection_skips_vm_and_tunnel_adapters() {
        let candidates = vec![
            ("vmnet8".to_string(), IpAddr::V4(Ipv4Addr::new(192, 168, 87, 1))),
            ("bridge100".to_string(), IpAddr::V4(Ipv4Addr::new(192, 168, 2, 1))),
            ("en1".to_string(), IpAddr::V4(Ipv4Addr::new(10, 1, 2, 3))),
        ];
        assert_eq!(
            select_lan_ipv4(candidates.into_iter()),
            Some(Ipv4Addr::new(10, 1, 2, 3))
        );
    }

    #[test]
    fn selection_falls_back_to_non_private_physical_adapter() {
        let candidates = vec![
            ("utun3".to_string(), IpAddr::V4(Ipv4Addr::new(198, 18, 0, 1))),
            ("en0".to_string(), IpAddr::V4(Ipv4Addr::new(100, 64, 11, 22))),
        ];
        assert_eq!(
            select_lan_ipv4(candidates.into_iter()),
            Some(Ipv4Addr::new(100, 64, 11, 22))
        );
    }

    #[test]
    fn selection_never_picks_container_or_vpn_overlays() {
        let candidates = vec![
            ("docker0".to_string(), IpAddr::V4(Ipv4Addr::new(172, 17, 0, 1))),
            ("veth2a4f".to_string(), IpAddr::V4(Ipv4Addr::new(172, 17, 0, 2))),
            ("cni0".to_string(), IpAddr::V4(Ipv4Addr::new(10, 244, 0, 1))),
            ("wg0".to_string(), IpAddr::V4(Ipv4Addr::new(10, 13, 37, 2))),
            ("tailscale0".to_string(), IpAddr::V4(Ipv4Addr::new(100, 101, 1, 1))),
            (
                "vEthernet (Default Switch)".to_string(),
                IpAddr::V4(Ipv4Addr::new(192, 168, 200, 1)),
            ),
            ("lo0".to_string(), IpAddr::V4(Ipv4Addr::LOCALHOST)),
        ];
        // Only virtual overlays present → publish nothing rather than hand a
        // bearer URL to containers or VPN peers.
        assert_eq!(select_lan_ipv4(candidates.into_iter()), None);
    }

    #[test]
    fn selection_accepts_windows_and_linux_physical_names() {
        let candidates = vec![
            ("Wi-Fi".to_string(), IpAddr::V4(Ipv4Addr::new(192, 168, 1, 9))),
            ("enp3s0".to_string(), IpAddr::V4(Ipv4Addr::new(10, 0, 0, 4))),
        ];
        assert_eq!(
            select_lan_ipv4(candidates.into_iter()),
            Some(Ipv4Addr::new(192, 168, 1, 9))
        );
    }

    #[test]
    fn credentialed_url_requires_bearer_and_http_origin() {
        let url = credentialed_access_url("http://192.168.1.8:5123", "tok").unwrap();
        assert!(url.ends_with("#access_token=tok"));
        assert!(credentialed_access_url("http://192.168.1.8:5123", "").is_none());
        assert!(credentialed_access_url("not-a-url", "tok").is_none());
    }
}
