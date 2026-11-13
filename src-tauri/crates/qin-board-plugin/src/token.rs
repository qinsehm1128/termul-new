//! Token generation and constant-time verification for the sidecar handshake.

/// 32 random bytes encoded as 64 lowercase hex characters.
pub fn generate() -> Result<String, getrandom::Error> {
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// Constant-time comparison (length is not secret: tokens are fixed size).
pub fn matches(expected: &str, candidate: &str) -> bool {
    let (a, b) = (expected.as_bytes(), candidate.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Extract and percent-decode the `token` query parameter from a request URI
/// query string (`a=1&token=...`).
pub fn from_query(query: Option<&str>) -> Option<String> {
    query?.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key == "token").then(|| percent_decode(value))
    })
}

/// True when the query carries a token equal to `expected`.
pub fn query_authorized(expected: &str, query: Option<&str>) -> bool {
    from_query(query).is_some_and(|candidate| matches(expected, &candidate))
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if let (Some(hi), Some(lo)) = (
                bytes.get(i + 1).copied().and_then(hex_value),
                bytes.get(i + 2).copied().and_then(hex_value),
            ) {
                out.push(hi << 4 | lo);
                i += 3;
                continue;
            }
        }
        out.push(if bytes[i] == b'+' { b' ' } else { bytes[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_tokens_are_hex_and_distinct() {
        let a = generate().unwrap();
        let b = generate().unwrap();
        assert_eq!(a.len(), 64);
        assert!(a.bytes().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }

    #[test]
    fn constant_time_match() {
        assert!(matches("abcd", "abcd"));
        assert!(!matches("abcd", "abce"));
        assert!(!matches("abcd", "abc"));
        assert!(!matches("abcd", ""));
    }

    #[test]
    fn query_parsing() {
        assert_eq!(from_query(Some("token=ab%2Fc")), Some("ab/c".into()));
        assert_eq!(from_query(Some("x=1&token=zz")), Some("zz".into()));
        assert_eq!(from_query(Some("x=1")), None);
        assert_eq!(from_query(None), None);
        assert_eq!(from_query(Some("token=%4")), Some("%4".into()));
        assert!(query_authorized("abc", Some("token=abc")));
        assert!(!query_authorized("abc", Some("token=abd")));
        assert!(!query_authorized("abc", Some("tok=abc")));
        assert!(!query_authorized("abc", None));
    }
}
