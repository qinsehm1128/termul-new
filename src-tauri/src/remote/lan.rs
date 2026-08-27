//! LAN address helpers for desktop shared-live publish URLs.

use std::net::{IpAddr, Ipv4Addr, UdpSocket};

/// Best-effort IPv4 the phone can reach on the same Wi-Fi.
///
/// Uses a UDP connect to learn the interface the OS would use for outbound
/// traffic. Never returns loopback, unspecified, or link-local addresses.
#[must_use]
pub fn discover_lan_ipv4() -> Option<Ipv4Addr> {
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
    fn credentialed_url_requires_bearer_and_http_origin() {
        let url = credentialed_access_url("http://192.168.1.8:5123", "tok").unwrap();
        assert!(url.ends_with("#access_token=tok"));
        assert!(credentialed_access_url("http://192.168.1.8:5123", "").is_none());
        assert!(credentialed_access_url("not-a-url", "tok").is_none());
    }
}
