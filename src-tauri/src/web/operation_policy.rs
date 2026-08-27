//! Shared HTTP/WS policy for host-local mutations.
//!
//! Locality is decided only from host-controlled [`IngressProvenance`].
//! Handlers must not reconstruct operator status from the TCP peer.

use crate::web::auth::IngressProvenance;

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
}

impl LocalOnlyOperation {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SetDefaultProject => "set_default_project",
            Self::SetCatalogOptIn => "set_catalog_opt_in",
            Self::InstallAcpAgent => "install_acp_agent",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PolicyDenial {
    pub code: &'static str,
    pub message: &'static str,
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
        })
    }
}
