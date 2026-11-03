use se_plugin_protocol::*;
use serde::Serialize;

tokio::task_local! {
    pub(crate) static CONTROL_DEADLINE: std::time::Instant;
}

pub(crate) fn deadline(timeout: std::time::Duration) -> std::time::Instant {
    let per_call = std::time::Instant::now() + timeout;
    CONTROL_DEADLINE
        .try_with(|deadline| per_call.min(*deadline))
        .unwrap_or(per_call)
}

pub(crate) fn tree(bytes: &[u8], view: &str) -> Result<UiTree, PluginError> {
    let tree = parse_ui_tree(bytes)?;
    if tree.view_id != view {
        return Err(error(
            PLUGIN_INVALID_UI,
            "callback returned another view's tree",
        ));
    }
    Ok(tree)
}

pub(crate) fn error(code: &str, message: &str) -> PluginError {
    PluginError::new(code, message)
}

pub(crate) fn io(error: impl std::fmt::Display) -> PluginError {
    PluginError::new(
        PLUGIN_IO,
        format!("plugin storage operation failed: {error}"),
    )
}

pub(crate) fn encode(value: &impl Serialize) -> Result<Vec<u8>, PluginError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|_| error(PLUGIN_INVALID_REQUEST, "cannot encode plugin request"))?;
    if bytes.len() > MAX_JSON_BYTES {
        return Err(error(PLUGIN_INVALID_REQUEST, "plugin JSON exceeds 256 KiB"));
    }
    Ok(bytes)
}

pub(crate) fn status(status: se_plugin_abi::SeStatus) -> Result<(), PluginError> {
    use se_plugin_abi::*;
    let code = match status {
        SEP_OK => return Ok(()),
        SEP_PANIC => PLUGIN_PANIC,
        SEP_BUSY => PLUGIN_BUSY,
        SEP_DEADLINE_EXCEEDED => PLUGIN_TIMEOUT,
        SEP_STALE_GENERATION => PLUGIN_STALE_GENERATION,
        SEP_REENTRANT => PLUGIN_SERVICE_CYCLE,
        SEP_ABI_UNSUPPORTED => PLUGIN_ABI_UNSUPPORTED,
        SEP_NOT_FOUND => PLUGIN_NOT_FOUND,
        SEP_UNAVAILABLE => PLUGIN_NOT_SUPPORTED,
        _ => PLUGIN_CALLBACK_ERROR,
    };
    Err(PluginError::new(
        code,
        format!("native callback returned status {status}"),
    ))
}

pub(crate) fn host_delivery_reply(
    bytes: &[u8],
    manifest: &se_plugin_protocol::Manifest,
) -> Result<se_plugin_protocol::HostDeliveryReply, PluginError> {
    let reply = se_plugin_protocol::parse_host_delivery_reply(bytes)
        .map_err(|_| error(PLUGIN_CALLBACK_ERROR, "invalid host delivery reply"))?;
    if reply.result.invalidate_views.iter().any(|view| {
        !manifest
            .contributes
            .views
            .iter()
            .any(|declared| &declared.id == view)
    }) {
        return Err(error(
            PLUGIN_CALLBACK_ERROR,
            "host delivery invalidated undeclared view",
        ));
    }
    Ok(reply)
}

pub(crate) fn fatal(error: &PluginError) -> bool {
    error.code == PLUGIN_TIMEOUT || error.code == PLUGIN_PANIC
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn host_delivery_invalidations_are_validated_all_or_nothing() {
        let manifest: se_plugin_protocol::Manifest = serde_json::from_value(serde_json::json!({
            "schemaVersion":1,"id":"test.reply","name":"Reply","version":"1.0.0",
            "abi":{"min":1,"max":1},"services":{"provides":[],"requires":[]},
            "contributes":{"commands":[],"views":[],"configuration":{"title":"Settings","properties":{}}}
        })).unwrap();
        assert!(host_delivery_reply(
            br#"{"schemaVersion":1,"result":{"invalidateViews":[]}}"#,
            &manifest
        )
        .is_ok());
        for bytes in [
            br#"{"schemaVersion":1,"result":{"invalidateViews":["missing"]}}"#.as_slice(),
            br#"{"schemaVersion":1,"result":{"invalidateViews":["duplicate","duplicate"]}}"#
                .as_slice(),
            br#"{"schemaVersion":1,"result":{"invalidateViews":[],"extra":true}}"#.as_slice(),
        ] {
            assert_eq!(
                host_delivery_reply(bytes, &manifest).unwrap_err().code,
                PLUGIN_CALLBACK_ERROR
            );
        }
    }
    #[test]
    fn unknown_status_is_not_success() {
        assert_eq!(status(999).unwrap_err().code, PLUGIN_CALLBACK_ERROR);
        assert_eq!(
            status(se_plugin_abi::SEP_PANIC).unwrap_err().code,
            PLUGIN_PANIC
        );
    }
}
