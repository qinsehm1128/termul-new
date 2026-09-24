//! Se-owned configuration-to-Core snapshot adapter.
//!
//! This module only reads an effective registry input. It does not know how to
//! persist the registry or secrets; those remain desktop/web authorities.

use std::{collections::BTreeMap, sync::Arc};

use serde_json::Value;
use tokio::sync::{Mutex, RwLock};

use super::{
    ConfigError, McpConfigSnapshot, McpControlPlaneConfig, McpCore, McpCoreError, McpDomainError,
    McpPersistedTransport, McpUpstreamConfig, McpUpstreamServer, McpUpstreamTransport, NamedSecret,
    RedactedSecret, MCP_CORE_CONTRACT_VERSION,
};

pub trait McpSecretResolver: Send + Sync {
    /// Resolve an already-classified secret token.
    ///
    /// Existing standalone callers use this method for compatibility. Desktop
    /// canonical application should call [`Self::resolve_named`] so inline
    /// values and keychain references remain distinguishable at the boundary.
    fn resolve(
        &self,
        server_id: &str,
        field: &str,
        name: &str,
        value: &str,
    ) -> Result<String, SnapshotError>;

    /// Resolve a persisted named secret without erasing whether it was inline
    /// or a reference. The default preserves the legacy resolver contract; a
    /// secure desktop resolver overrides this method to send only `ref` tokens
    /// to the keychain owner while returning inline values directly.
    fn resolve_named(
        &self,
        server_id: &str,
        field: &str,
        secret: &NamedSecret,
    ) -> Result<String, SnapshotError> {
        let token = secret.resolver_token().map_err(config_error)?;
        self.resolve(server_id, field, &secret.name, token)
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct InlineSecretResolver;

impl McpSecretResolver for InlineSecretResolver {
    fn resolve(
        &self,
        _server_id: &str,
        _field: &str,
        _name: &str,
        value: &str,
    ) -> Result<String, SnapshotError> {
        Ok(value.to_owned())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SnapshotError {
    InvalidRegistry(String),
    SecretResolution {
        server_id: String,
        field: String,
        message: String,
    },
    InvalidSnapshot(McpCoreError),
    StaleRevision {
        accepted: u64,
        requested: u64,
    },
    ApplyFailed(McpDomainError),
    Unavailable(String),
}

impl std::fmt::Display for SnapshotError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidRegistry(message) => write!(f, "invalid MCP registry: {message}"),
            Self::SecretResolution {
                server_id,
                field,
                message,
            } => write!(
                f,
                "failed to resolve MCP secret for {server_id}.{field}: {message}"
            ),
            Self::InvalidSnapshot(error) => write!(f, "invalid MCP snapshot: {}", error.message),
            Self::StaleRevision {
                accepted,
                requested,
            } => write!(
                f,
                "MCP snapshot revision {requested} is not newer than {accepted}"
            ),
            Self::ApplyFailed(error) => write!(f, "MCP snapshot apply failed: {error}"),
            Self::Unavailable(message) => write!(f, "MCP snapshot runtime unavailable: {message}"),
        }
    }
}

impl std::error::Error for SnapshotError {}

pub fn build_snapshot(
    registry: &Value,
    revision: u64,
    resolver: &dyn McpSecretResolver,
) -> Result<McpConfigSnapshot, SnapshotError> {
    let parsed = McpControlPlaneConfig::from_stored_json(registry).map_err(config_error)?;
    snapshot_from_config(&parsed.config, revision, resolver)
}

/// Build a runtime snapshot from the canonical project config.
///
/// `revision` is the runtime apply revision. Persistence revision lives on the
/// control-plane document; callers typically pass `config.revision`.
pub fn snapshot_from_config(
    config: &McpControlPlaneConfig,
    revision: u64,
    resolver: &dyn McpSecretResolver,
) -> Result<McpConfigSnapshot, SnapshotError> {
    let mut servers = Vec::with_capacity(config.upstreams.len());
    for upstream in &config.upstreams {
        servers.push(build_server(upstream, resolver)?);
    }
    let snapshot = McpConfigSnapshot {
        contract_version: MCP_CORE_CONTRACT_VERSION,
        revision,
        servers,
    };
    snapshot
        .validate()
        .map_err(SnapshotError::InvalidSnapshot)?;
    Ok(snapshot)
}

fn config_error(error: ConfigError) -> SnapshotError {
    SnapshotError::InvalidRegistry(error.to_string())
}

fn build_server(
    upstream: &McpUpstreamConfig,
    resolver: &dyn McpSecretResolver,
) -> Result<McpUpstreamServer, SnapshotError> {
    let transport = match &upstream.transport {
        McpPersistedTransport::Stdio { command, args, env } => McpUpstreamTransport::Stdio {
            command: command.clone(),
            args: args.clone(),
            env: resolve_secrets(&upstream.id, "env", env, resolver)?,
        },
        McpPersistedTransport::Http { url, headers } => McpUpstreamTransport::StreamableHttp {
            url: url.clone(),
            headers: resolve_secrets(&upstream.id, "headers", headers, resolver)?,
        },
        McpPersistedTransport::Sse { .. } => {
            return Err(SnapshotError::InvalidRegistry(
                "legacy SSE upstreams are not supported by MCP Core; use type=http".into(),
            ));
        }
    };
    Ok(McpUpstreamServer {
        id: upstream.id.clone(),
        enabled: upstream.enabled,
        transport,
    })
}

fn resolve_secrets(
    server_id: &str,
    field: &str,
    secrets: &[NamedSecret],
    resolver: &dyn McpSecretResolver,
) -> Result<BTreeMap<String, RedactedSecret>, SnapshotError> {
    let mut resolved = BTreeMap::new();
    for secret in secrets {
        let value =
            resolver
                .resolve_named(server_id, field, secret)
                .map_err(|error| match error {
                    SnapshotError::SecretResolution { .. } => error,
                    other => SnapshotError::SecretResolution {
                        server_id: server_id.to_owned(),
                        field: field.to_owned(),
                        message: other.to_string(),
                    },
                })?;
        resolved.insert(secret.name.clone(), RedactedSecret::new(value));
    }
    Ok(resolved)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotApplyReceipt {
    pub revision: u64,
    pub replaced_last_good: bool,
}

struct SnapshotState {
    accepted_revision: u64,
    last_good: Option<McpConfigSnapshot>,
}

pub struct McpSnapshotController {
    core: Arc<McpCore>,
    state: RwLock<SnapshotState>,
    update_lock: Mutex<()>,
}

impl std::fmt::Debug for McpSnapshotController {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpSnapshotController")
            .field("core", &"instance-owned")
            .field("state", &"last-known-good")
            .finish_non_exhaustive()
    }
}

impl McpSnapshotController {
    pub fn new(core: Arc<McpCore>) -> Self {
        Self {
            core,
            state: RwLock::new(SnapshotState {
                accepted_revision: 0,
                last_good: None,
            }),
            update_lock: Mutex::new(()),
        }
    }

    pub async fn apply(
        &self,
        snapshot: McpConfigSnapshot,
    ) -> Result<SnapshotApplyReceipt, SnapshotError> {
        snapshot
            .validate()
            .map_err(SnapshotError::InvalidSnapshot)?;
        let _update = self.update_lock.lock().await;
        let accepted = self.state.read().await.accepted_revision;
        if snapshot.revision <= accepted {
            return Err(SnapshotError::StaleRevision {
                accepted,
                requested: snapshot.revision,
            });
        }
        self.core
            .apply_snapshot(&snapshot)
            .await
            .map_err(SnapshotError::ApplyFailed)?;
        let revision = snapshot.revision;
        let mut state = self.state.write().await;
        state.accepted_revision = revision;
        state.last_good = Some(snapshot);
        Ok(SnapshotApplyReceipt {
            revision,
            replaced_last_good: true,
        })
    }

    pub async fn apply_registry(
        &self,
        registry: &Value,
        revision: u64,
        resolver: &dyn McpSecretResolver,
    ) -> Result<SnapshotApplyReceipt, SnapshotError> {
        let parsed = McpControlPlaneConfig::from_stored_json(registry).map_err(config_error)?;
        let snapshot = snapshot_from_config(&parsed.config, revision, resolver)?;
        let receipt = self.apply(snapshot).await?;
        self.core
            .apply_built_in_config(&parsed.config.built_ins)
            .await;
        Ok(receipt)
    }

    pub async fn apply_config(
        &self,
        config: &McpControlPlaneConfig,
        resolver: &dyn McpSecretResolver,
    ) -> Result<SnapshotApplyReceipt, SnapshotError> {
        let snapshot = snapshot_from_config(config, config.revision, resolver)?;
        let receipt = self.apply(snapshot).await?;
        self.core.apply_built_in_config(&config.built_ins).await;
        Ok(receipt)
    }

    /// Reset only the revision fence when the desktop switches canonical
    /// project scope. The current last-good snapshot remains live until the
    /// replacement applies successfully, preserving rollback on read/secret/
    /// connection failure.
    pub async fn reset_revision_for_project_scope(&self) {
        let _update = self.update_lock.lock().await;
        self.state.write().await.accepted_revision = 0;
    }

    pub async fn accepted_revision(&self) -> u64 {
        self.state.read().await.accepted_revision
    }

    pub async fn last_good(&self) -> Option<McpConfigSnapshot> {
        self.state.read().await.last_good.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;

    struct FailingResolver;

    impl McpSecretResolver for FailingResolver {
        fn resolve(
            &self,
            server_id: &str,
            field: &str,
            _name: &str,
            _value: &str,
        ) -> Result<String, SnapshotError> {
            Err(SnapshotError::SecretResolution {
                server_id: server_id.into(),
                field: field.into(),
                message: "secure store unavailable".into(),
            })
        }
    }

    struct RecordingResolver(StdMutex<Vec<(String, String, String)>>);

    impl McpSecretResolver for RecordingResolver {
        fn resolve(
            &self,
            server_id: &str,
            field: &str,
            name: &str,
            value: &str,
        ) -> Result<String, SnapshotError> {
            self.0.lock().unwrap().push((
                server_id.into(),
                field.into(),
                format!("{name}={value}"),
            ));
            Ok(value.into())
        }
    }

    fn registry() -> Value {
        serde_json::json!([
            {
                "id": "local",
                "type": "stdio",
                "command": "fixture",
                "args": ["--stdio"],
                "env": [{"name": "TOKEN", "value": "secret"}],
                "enabled": true
            },
            {
                "id": "remote",
                "type": "http",
                "url": "http://127.0.0.1:43123/mcp",
                "headers": {"Authorization": "Bearer secret"},
                "enabled": false
            }
        ])
    }

    #[test]
    fn desktop_array_pairs_and_standalone_object_pairs_have_same_snapshot() {
        let desktop = registry();
        let standalone = serde_json::json!([
            {
                "id": "local",
                "type": "stdio",
                "command": "fixture",
                "args": ["--stdio"],
                "env": {"TOKEN": "secret"},
                "enabled": true
            },
            {
                "id": "remote",
                "type": "http",
                "url": "http://127.0.0.1:43123/mcp",
                "headers": {"Authorization": "Bearer secret"},
                "enabled": false
            }
        ]);
        let left = build_snapshot(&desktop, 1, &InlineSecretResolver).unwrap();
        let right = build_snapshot(&standalone, 1, &InlineSecretResolver).unwrap();
        assert_eq!(left, right);
        assert!(!format!("{left:?}").contains("secret"));
    }

    #[test]
    fn secure_store_failure_rejects_without_returning_secret() {
        let error = build_snapshot(&registry(), 1, &FailingResolver).unwrap_err();
        let text = error.to_string();
        assert!(text.contains("secure store unavailable"));
        assert!(!text.contains("Bearer secret"));
    }

    #[test]
    fn resolver_is_the_only_boundary_that_receives_secret_values() {
        let resolver = RecordingResolver(StdMutex::new(Vec::new()));
        let snapshot = build_snapshot(&registry(), 1, &resolver).unwrap();
        assert_eq!(resolver.0.lock().unwrap().len(), 2);
        assert_eq!(snapshot.servers.len(), 2);
    }

    #[test]
    fn unsupported_sse_is_rejected_explicitly() {
        let value = serde_json::json!([{
            "id": "legacy",
            "type": "sse",
            "url": "http://127.0.0.1:43123/sse"
        }]);
        let error = build_snapshot(&value, 1, &InlineSecretResolver).unwrap_err();
        assert!(error.to_string().contains("legacy SSE"));
    }

    #[test]
    fn canonical_object_and_legacy_array_produce_the_same_snapshot() {
        let array = registry();
        let canonical = McpControlPlaneConfig::from_stored_json(&array)
            .unwrap()
            .config
            .to_canonical_json()
            .unwrap();
        let left = build_snapshot(&array, 4, &InlineSecretResolver).unwrap();
        let right = build_snapshot(&canonical, 4, &InlineSecretResolver).unwrap();
        assert_eq!(left, right);
        assert_eq!(left.revision, 4);
        assert!(!format!("{left:?}").contains("secret"));
    }

    #[test]
    fn secret_references_are_resolved_without_logging_inline_values() {
        let resolver = RecordingResolver(StdMutex::new(Vec::new()));
        let config = serde_json::json!({
            "schemaVersion": 1,
            "revision": 2,
            "upstreams": [{
                "id": "remote",
                "name": "Remote",
                "type": "http",
                "url": "http://127.0.0.1:43123/mcp",
                "headers": [{
                    "name": "Authorization",
                    "ref": "mcp/remote/authorization"
                }]
            }]
        });
        let snapshot = build_snapshot(&config, 2, &resolver).unwrap();
        assert_eq!(
            resolver.0.lock().unwrap().as_slice(),
            &[(
                "remote".into(),
                "headers".into(),
                "Authorization=mcp/remote/authorization".into()
            )]
        );
        assert_eq!(snapshot.servers.len(), 1);
        assert!(!format!("{snapshot:?}").contains("mcp/remote/authorization"));
    }
}
