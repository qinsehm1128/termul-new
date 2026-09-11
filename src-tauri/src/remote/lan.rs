//! LAN address helpers for desktop shared-live publish URLs.

use std::net::{IpAddr, Ipv4Addr, UdpSocket};

/// Best-effort IPv4 the phone can reach on the same Wi-Fi.
///
/// A UDP connect follows the default route — which, with a proxy TUN adapter
/// (Clash/Surge/sing-box fake-ip), is the virtual interface, so the probe
/// returns something like `198.18.0.1` that no physical network carries.
/// Interface enumeration is therefore primary: pick the best usable address
/// from a physical adapter, preferring RFC1918 ranges the phone can route
/// to. The route-based probe stays as a fallback for odd interface setups.
#[must_use]
pub fn discover_lan_ipv4() -> Option<Ipv4Addr> {
    if let Ok(addrs) = if_addrs::get_if_addrs() {
        let candidates = addrs.into_iter().map(|addr| {
            let ip = addr.ip();
            (addr.name, ip)
        });
        if let Some(best) = select_lan_ipv4(candidates) {
            return Some(best);
        }
    }
    discover_lan_ipv4_via_route()
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

/// TUN/TAP adapters belong to VPNs and proxies; awdl/llw are Apple
/// peer-to-peer links; ap/anpi/bridge are software bridges; vnic/vmnet/
/// virbr host VM NATs; gif/stf are tunnels. None of them carry the LAN the
/// phone is on.
fn is_physical_adapter(name: &str) -> bool {
    let lowered = name.to_ascii_lowercase();
    !lowered.starts_with("utun")
        && !lowered.starts_with("tun")
        && !lowered.starts_with("tap")
        && !lowered.starts_with("awdl")
        && !lowered.starts_with("llw")
        && !lowered.starts_with("ap")
        && !lowered.starts_with("anpi")
        && !lowered.starts_with("bridge")
        && !lowered.starts_with("vnic")
        && !lowered.starts_with("vmnet")
        && !lowered.starts_with("virbr")
        && !lowered.starts_with("gif")
        && !lowered.starts_with("stf")
}

fn discover_lan_ipv4_via_route() -> Option<Ipv4Addr> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("1.1.1.1:80").ok()?;
    match socket.local_addr().ok()?.ip() {
        IpAddr::V4(ip) if is_usable_lan_v4(ip) => Some(ip),
        _ => None,
    }
}

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
    fn credentialed_url_requires_bearer_and_http_origin() {
        let url = credentialed_access_url("http://192.168.1.8:5123", "tok").unwrap();
        assert!(url.ends_with("#access_token=tok"));
        assert!(credentialed_access_url("http://192.168.1.8:5123", "").is_none());
        assert!(credentialed_access_url("not-a-url", "tok").is_none());
    }
}
