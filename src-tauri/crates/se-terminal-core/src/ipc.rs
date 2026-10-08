//! App-level Core identity on top of the `se-ipc` wire protocol.
//!
//! Build identity and capabilities describe this application's components,
//! so they live here rather than in the protocol crate.

pub use se_ipc::*;

pub fn component_build_id(role: CoreRole) -> String {
    let configured = match role {
        CoreRole::Gui => option_env!("TERMUL_GUI_BUILD_ID"),
        CoreRole::TerminalCore => option_env!("TERMUL_TERMINAL_CORE_BUILD_ID"),
        CoreRole::AcpCore => option_env!("TERMUL_ACP_CORE_BUILD_ID"),
    };
    configured
        .map(str::to_owned)
        .unwrap_or_else(|| format!("{}:{}", env!("CARGO_PKG_VERSION"), role.endpoint_name()))
}

pub fn component_capabilities(role: CoreRole) -> Vec<String> {
    match role {
        CoreRole::Gui => vec!["gui-client".to_string()],
        CoreRole::TerminalCore => vec![
            "terminal-stream-replay".to_string(),
            "terminal-active-resource-count".to_string(),
        ],
        CoreRole::AcpCore => vec![
            "acp-event-replay".to_string(),
            "acp-active-resource-count".to_string(),
            "acp-single-writer".to_string(),
        ],
    }
}

pub fn validate_hello(
    hello: &CoreHello,
    expected_role: CoreRole,
) -> Result<CoreHelloAck, CoreError> {
    validate_hello_with_runtime(hello, expected_role, 0)
}

pub fn validate_hello_with_runtime(
    hello: &CoreHello,
    expected_role: CoreRole,
    active_resources: u32,
) -> Result<CoreHelloAck, CoreError> {
    if hello.role != expected_role {
        return Err(CoreError::Unauthorized);
    }

    Ok(CoreHelloAck {
        role: expected_role,
        protocol_version: negotiate_protocol(&hello.protocol_versions)?,
        component_build_id: Some(component_build_id(expected_role)),
        capabilities: component_capabilities(expected_role),
        active_resources,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_wrong_role_without_leaking_endpoint_state() {
        let hello = CoreHello {
            role: CoreRole::Gui,
            protocol_versions: vec![CURRENT_PROTOCOL_VERSION],
            client_name: "test".to_string(),
        };
        assert_eq!(
            validate_hello(&hello, CoreRole::TerminalCore),
            Err(CoreError::Unauthorized)
        );
        assert_eq!(CoreError::Unauthorized.client_message(), "unauthorized");
    }

    #[test]
    fn runtime_ack_exposes_identity_capabilities_and_active_resources() {
        let hello = CoreHello {
            role: CoreRole::TerminalCore,
            protocol_versions: vec![CURRENT_PROTOCOL_VERSION],
            client_name: "test".to_string(),
        };
        let ack = validate_hello_with_runtime(&hello, CoreRole::TerminalCore, 3).unwrap();
        assert!(ack.component_build_id.is_some());
        assert!(ack
            .capabilities
            .contains(&"terminal-active-resource-count".to_string()));
        assert_eq!(ack.active_resources, 3);
    }
}
