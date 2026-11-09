//! Shared HTTP/WS policy for host-local mutations.
//!
//! Locality is decided only from host-controlled [`IngressProvenance`].
//! Handlers must not reconstruct operator status from the TCP peer.

use crate::web::auth::{IngressProvenance, RemoteAccessAuthority, RemotePrincipal};

pub const FORBIDDEN: &str = "FORBIDDEN";
pub const VALIDATION_ERROR: &str = "VALIDATION_ERROR";
pub const PERSIST_FAILED: &str = "PERSIST_FAILED";
pub const NOT_FOUND: &str = "NOT_FOUND";

/// Host mutations that require [`IngressProvenance::LocalOperator`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalOnlyOperation {
    SetDefaultProject,
    SetCatalogOptIn,
    InstallAcpAgent,
    SpawnAgent,
    CreateSession,
    StoreAccess,
    PermissionPolicy,
    KillAgent,
    SkillsMutation,
}

impl LocalOnlyOperation {
    #[must_use]
    pub const fn scope(self) -> &'static str {
        match self {
            Self::SetDefaultProject | Self::SpawnAgent | Self::CreateSession | Self::KillAgent => {
                "agent.run"
            }
            Self::SetCatalogOptIn
            | Self::InstallAcpAgent
            | Self::StoreAccess
            | Self::PermissionPolicy => "manage",
            Self::SkillsMutation => "files.write",
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SetDefaultProject => "set_default_project",
            Self::SetCatalogOptIn => "set_catalog_opt_in",
            Self::InstallAcpAgent => "install_acp_agent",
            Self::SpawnAgent => "spawn_agent",
            Self::CreateSession => "create_session",
            Self::StoreAccess => "store_access",
            Self::PermissionPolicy => "set_permission_policy",
            Self::KillAgent => "kill_agent",
            Self::SkillsMutation => "skills_mutation",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PolicyDenial {
    pub code: &'static str,
    pub message: &'static str,
    pub capability: &'static str,
}

/// Paired identities are checked against current device state on every call.
/// Legacy public credentials retain the original local-only restrictions.
/// Provenance is still host-controlled; no client claim can grant local access.
pub fn authorize_device_operation(
    authority: &RemoteAccessAuthority,
    provenance: IngressProvenance,
    principal: Option<&RemotePrincipal>,
    operation: LocalOnlyOperation,
) -> Result<(), PolicyDenial> {
    if let Some(principal) = principal.filter(|principal| !principal.is_legacy()) {
        return authority
            .require_scope(principal, operation.scope())
            .map_err(|error| PolicyDenial {
                code: error.code(),
                message: "device does not have permission for this operation",
                capability: operation.scope(),
            });
    }
    authorize_local_only(provenance, operation)
}

/// HTTP callers keep isolated local-route tests compatible while failing closed
/// if a paired identity is ever supplied without its validating authority.
pub fn authorize_request_operation(
    provenance: IngressProvenance,
    principal: Option<&RemotePrincipal>,
    authority: Option<&RemoteAccessAuthority>,
    operation: LocalOnlyOperation,
) -> Result<(), PolicyDenial> {
    match authority {
        Some(authority) => authorize_device_operation(authority, provenance, principal, operation),
        None if principal.is_some_and(|principal| !principal.is_legacy()) => Err(PolicyDenial {
            code: FORBIDDEN,
            message: "device authority is unavailable",
            capability: operation.scope(),
        }),
        None => authorize_local_only(provenance, operation),
    }
}

/// Existing HTTP IPC envelope, enriched only when a policy denial needs a scope.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct PolicyIpcBody<T> {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capability: Option<String>,
}

impl<T> From<crate::web::fs_api::IpcBody<T>> for PolicyIpcBody<T> {
    fn from(body: crate::web::fs_api::IpcBody<T>) -> Self {
        Self {
            success: body.success,
            data: body.data,
            error: body.error,
            code: body.code,
            capability: None,
        }
    }
}

impl<T> PolicyIpcBody<T> {
    pub fn ok(data: T) -> Self {
        crate::web::fs_api::IpcBody::ok(data).into()
    }
    pub fn err(message: impl Into<String>, code: impl Into<String>) -> Self {
        crate::web::fs_api::IpcBody::err(message, code).into()
    }
    pub fn denied(denial: PolicyDenial) -> Self {
        let mut body = Self::err(denial.message, denial.code);
        body.capability = Some(denial.capability.to_owned());
        body
    }
}

/// Require LocalOperator for the listed host mutations. Public-tunnel clients
/// receive a stable FORBIDDEN without persisting host state.
pub fn authorize_local_only(
    provenance: IngressProvenance,
    _operation: LocalOnlyOperation,
) -> Result<(), PolicyDenial> {
    if provenance.allows_local_operator_mutation() {
        Ok(())
    } else {
        Err(PolicyDenial {
            code: FORBIDDEN,
            message: "host mutation requires local-operator ingress",
            capability: _operation.scope(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OPERATIONS: [LocalOnlyOperation; 9] = [
        LocalOnlyOperation::SetDefaultProject,
        LocalOnlyOperation::SetCatalogOptIn,
        LocalOnlyOperation::InstallAcpAgent,
        LocalOnlyOperation::SpawnAgent,
        LocalOnlyOperation::CreateSession,
        LocalOnlyOperation::KillAgent,
        LocalOnlyOperation::StoreAccess,
        LocalOnlyOperation::PermissionPolicy,
        LocalOnlyOperation::SkillsMutation,
    ];

    #[test]
    fn scoped_ipc_preserves_public_fields_and_deserialization() {
        let body = PolicyIpcBody::<String>::denied(PolicyDenial {
            code: FORBIDDEN,
            message: "denied",
            capability: "manage",
        });
        let value = serde_json::to_value(body).unwrap();
        let body: PolicyIpcBody<String> = serde_json::from_value(value.clone()).unwrap();
        assert!(!body.success);
        assert_eq!(body.code.as_deref(), Some(FORBIDDEN));
        assert_eq!(body.capability.as_deref(), Some("manage"));
        let legacy: crate::web::fs_api::IpcBody<String> = serde_json::from_value(value).unwrap();
        assert_eq!(legacy.error.as_deref(), Some("denied"));
        let body: PolicyIpcBody<String> =
            serde_json::from_value(serde_json::json!({"success":true,"data":"hello"})).unwrap();
        assert_eq!(body.data.unwrap(), "hello");
    }

    #[test]
    fn paired_policy_checks_live_grants_and_never_grants_manage() {
        let authority = RemoteAccessAuthority::desktop_memory();
        let _credential = authority.adopt_or_issue_desktop_credential(None).unwrap();
        let paired = authority
            .pair_device(crate::web::devices::PairRequest {
                code: authority.issue_pairing_code().unwrap().code,
                name: "Policy test".into(),
                platform: "test".into(),
            })
            .unwrap();
        let principal = authority.verify_bearer(&paired.device_token).unwrap();
        for operation in OPERATIONS {
            let result = authorize_device_operation(
                &authority,
                IngressProvenance::PublicTunnel,
                Some(&principal),
                operation,
            );
            if operation.scope() != "manage" {
                assert!(result.is_ok(), "{operation:?}");
            } else {
                let denial = result.unwrap_err();
                assert_eq!(denial.code, FORBIDDEN);
                assert_eq!(denial.capability, "manage");
            }
        }
        authority
            .set_device_scopes(&paired.device_id, vec!["view".into()])
            .unwrap();
        let denial = authorize_device_operation(
            &authority,
            IngressProvenance::PublicTunnel,
            Some(&principal),
            LocalOnlyOperation::SpawnAgent,
        )
        .unwrap_err();
        assert_eq!(denial.capability, "agent.run");
        authority.revoke_device(&paired.device_id).unwrap();
        assert_eq!(
            authorize_device_operation(
                &authority,
                IngressProvenance::PublicTunnel,
                Some(&principal),
                LocalOnlyOperation::SpawnAgent
            )
            .unwrap_err()
            .code,
            "DEVICE_REVOKED"
        );
    }

    #[test]
    fn legacy_public_gates_stay_denied_and_local_gates_stay_open() {
        for operation in OPERATIONS {
            assert!(authorize_local_only(IngressProvenance::PublicTunnel, operation).is_err());
            assert!(authorize_local_only(IngressProvenance::LocalOperator, operation).is_ok());
        }
    }
}
