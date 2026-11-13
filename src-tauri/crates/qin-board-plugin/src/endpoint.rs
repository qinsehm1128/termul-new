//! Parsing of the sidecar's endpoint announcement line.
use serde_json::Value;

pub const ENDPOINT_PREFIX: &str = "QIN_BOARD_ENDPOINT ";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    pub port: u16,
    pub token: String,
}

/// Parse one log line `QIN_BOARD_ENDPOINT {"port":P,"token":"T"}`.
pub fn parse_line(line: &str) -> Option<Endpoint> {
    let rest = line.trim_end().strip_prefix(ENDPOINT_PREFIX)?;
    let value: Value = serde_json::from_str(rest.trim()).ok()?;
    let port = u16::try_from(value["port"].as_u64()?).ok()?;
    let token = value["token"].as_str()?;
    if port == 0 || token.is_empty() || token.len() > 512 {
        return None;
    }
    Some(Endpoint {
        port,
        token: token.to_owned(),
    })
}

/// Last valid announcement among `se.host.process logs` result lines.
pub fn latest_from_logs(logs: &Value) -> Option<Endpoint> {
    logs["lines"]
        .as_array()?
        .iter()
        .rev()
        .filter_map(|entry| entry["text"].as_str())
        .find_map(parse_line)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn picks_last_valid_line() {
        let logs = json!({"lines":[
            {"text":"QIN_BOARD_ENDPOINT {\"port\":1000,\"token\":\"aa\"}"},
            {"text":"noise"},
            {"text":"QIN_BOARD_ENDPOINT {\"port\":2000,\"token\":\"bb\"}\n"},
            {"text":"QIN_BOARD_ENDPOINT garbage"}
        ]});
        assert_eq!(
            latest_from_logs(&logs),
            Some(Endpoint {
                port: 2000,
                token: "bb".into()
            })
        );
        assert_eq!(latest_from_logs(&json!({"lines":[]})), None);
        assert_eq!(
            parse_line("QIN_BOARD_ENDPOINT {\"port\":0,\"token\":\"x\"}"),
            None
        );
    }
}
