//! Revisioned passive workspace projection stored inside each canonical Conversation directory.
//!
//! ConversationRepository remains the canonical writer boundary. This service never owns, creates,
//! attaches, detaches, or terminates a PTY and persists only non-owning resource references.

use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::Utc;
use parking_lot::Mutex as ParkingMutex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::conversation::contracts::{format_created_at_utc, ConversationId};
use crate::conversation::migration::{
    LegacyInventoryV1, LegacySourceKind, MigrationJournalV1, MigrationMapV1, RecoveryAction,
    RecoveryActionErrorCode, RecoveryActionResult, RecoveryItemV1, RecoveryKind,
    RecoveryProvenanceV1, RecoveryQueueV1, RecoverySeverity, RecoveryStatus,
    ResolveRecoveryItemRequest, INVENTORY_FILE, MIGRATION_JOURNAL_FILE, MIGRATION_MAP_FILE,
    RECOVERY_ITEMS_FILE,
};
use crate::conversation::repository::{
    ConversationRepository, RepositoryError, RepositoryOpenReport,
};
use crate::conversation::workspace_projection::{
    LegacyWorkspaceProjector, WorkspaceProjectionOutcome,
};
use crate::conversation::write_authority::{ConversationMutation, ConversationWriter};

pub const SESSION_WORKSPACE_SCHEMA_VERSION: u32 = 1;
const FALLBACK_RECOVERY_OPERATION: &str = "workspace-recovery-v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionWorkspacePaneDirection {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionWorkspaceSplitNode {
    pub id: String,
    pub direction: SessionWorkspacePaneDirection,
    pub children: Vec<SessionWorkspacePaneNode>,
    pub sizes: Vec<f64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionWorkspaceLeafNode {
    pub id: String,
    #[serde(default)]
    pub terminal_ids: Vec<String>,
    #[serde(default)]
    pub editor_ids: Vec<String>,
    pub active_tab_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum SessionWorkspacePaneNode {
    Split(SessionWorkspaceSplitNode),
    Leaf(SessionWorkspaceLeafNode),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum SessionWorkspaceResourceDescriptor {
    Terminal {
        /// PtyManager-owned resource id.
        #[serde(rename = "terminalId")]
        terminal_id: String,
        /// Optional renderer record id used only to rebuild visible topology.
        #[serde(
            rename = "terminalRecordId",
            default,
            skip_serializing_if = "Option::is_none"
        )]
        terminal_record_id: Option<String>,
        #[serde(rename = "conversationId")]
        conversation_id: ConversationId,
    },
    Editor {
        #[serde(rename = "editorId")]
        editor_id: String,
        #[serde(rename = "filePath")]
        file_path: String,
    },
}

pub type TerminalResourceDescriptor = SessionWorkspaceResourceDescriptor;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "camelCase", deny_unknown_fields)]
pub enum SessionWorkspaceProjectionState {
    Native,
    Projected {
        #[serde(rename = "sourcePath", alias = "source_path")]
        source_path: String,
        #[serde(rename = "sourceSha256", alias = "source_sha256")]
        source_sha256: String,
        #[serde(rename = "projectedResourceCount", alias = "projected_resource_count")]
        projected_resource_count: usize,
        #[serde(
            rename = "unresolvedResourceCount",
            alias = "unresolved_resource_count"
        )]
        unresolved_resource_count: usize,
    },
    RecoveryRequired {
        #[serde(rename = "recoveryIds", alias = "recovery_ids")]
        recovery_ids: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionWorkspaceV1 {
    pub schema_version: u32,
    pub conversation_id: ConversationId,
    pub revision: u64,
    pub updated_at_utc: String,
    pub update_identity: Option<String>,
    pub topology: Option<SessionWorkspacePaneNode>,
    pub active_pane_id: Option<String>,
    #[serde(default)]
    pub resources: Vec<SessionWorkspaceResourceDescriptor>,
    pub projection_state: SessionWorkspaceProjectionState,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum SessionWorkspaceLoadOutcome {
    Missing {
        #[serde(rename = "conversationId", alias = "conversation_id")]
        conversation_id: ConversationId,
    },
    Loaded {
        workspace: Box<SessionWorkspaceV1>,
    },
    RecoveryRequired {
        #[serde(rename = "conversationId", alias = "conversation_id")]
        conversation_id: ConversationId,
        #[serde(rename = "recoveryItems", alias = "recovery_items")]
        recovery_items: Vec<RecoveryItemV1>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum SessionWorkspaceWriteOutcome {
    Updated {
        revision: u64,
        #[serde(rename = "updatedAtUtc", alias = "updated_at_utc")]
        updated_at_utc: String,
    },
    Conflict {
        #[serde(rename = "currentRevision", alias = "current_revision")]
        current_revision: u64,
        #[serde(rename = "currentUpdatedAtUtc", alias = "current_updated_at_utc")]
        current_updated_at_utc: String,
        #[serde(rename = "currentUpdateIdentity", alias = "current_update_identity")]
        current_update_identity: Option<String>,
    },
    RecoveryRequired {
        #[serde(rename = "recoveryItems", alias = "recovery_items")]
        recovery_items: Vec<RecoveryItemV1>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SessionWorkspaceErrorCode {
    ConversationInvalidId,
    ConversationNotFound,
    ConversationConflict,
    ConversationRecoveryRequired,
    ConversationDurabilityFailed,
    ValidationError,
    RecoveryNotFound,
    MigrationIdempotencyConflict,
    SessionWorkspaceUnavailable,
    SessionWorkspaceRecoveryRequired,
    LegacyCompatibilityReadOnly,
}

impl SessionWorkspaceErrorCode {
    #[must_use]
    pub fn as_str(self) -> String {
        serde_json::to_value(self)
            .ok()
            .and_then(|value| value.as_str().map(str::to_string))
            .unwrap_or_else(|| "CONVERSATION_RECOVERY_REQUIRED".to_string())
    }
}

#[derive(Debug)]
pub struct SessionWorkspaceError {
    pub code: SessionWorkspaceErrorCode,
    pub operation: &'static str,
    pub conversation_id: Option<ConversationId>,
    pub detail: String,
}

impl fmt::Display for SessionWorkspaceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code.as_str(), self.detail)
    }
}

impl std::error::Error for SessionWorkspaceError {}

pub type Result<T> = std::result::Result<T, SessionWorkspaceError>;

pub const TERMINAL_RESOURCE_ROLLBACK_FAILED: &str = "TERMINAL_RESOURCE_ROLLBACK_FAILED";
pub const TERMINAL_TERMINATE_FAILED: &str = "TERMINATE_FAILED";

/// Safe compound identity returned when passive-ref admission fails and PTY rollback also fails.
/// It deliberately contains no claim, environment, argv, cwd contents, or terminal output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalResourceRollbackFailure {
    pub terminal_id: String,
    pub conversation_id: ConversationId,
    pub primary_code: String,
    pub rollback_code: String,
}

impl TerminalResourceRollbackFailure {
    #[must_use]
    pub fn wire_detail(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| {
            format!(
                "{{\"terminalId\":\"{}\",\"conversationId\":\"{}\",\"primaryCode\":\"{}\",\"rollbackCode\":\"{}\"}}",
                self.terminal_id, self.conversation_id, self.primary_code, self.rollback_code
            )
        })
    }
}

#[derive(Clone)]
pub struct SessionWorkspaceService {
    writer: Arc<ConversationWriter>,
    repository: Arc<ConversationRepository>,
    repository_recovery_items: Arc<ParkingMutex<Vec<RecoveryItemV1>>>,
}

impl SessionWorkspaceService {
    #[must_use]
    pub fn new(writer: Arc<ConversationWriter>) -> Self {
        let repository = Arc::clone(writer.repository());
        let repository_recovery_items = repository_recovery_items(&repository.recovery_report());
        Self {
            writer,
            repository,
            repository_recovery_items: Arc::new(ParkingMutex::new(repository_recovery_items)),
        }
    }

    #[must_use]
    pub fn repository(&self) -> &Arc<ConversationRepository> {
        &self.repository
    }

    #[must_use]
    pub fn writer(&self) -> &Arc<ConversationWriter> {
        &self.writer
    }

    pub fn ensure_terminal_ref_writable(
        &self,
        conversation_id: ConversationId,
        add: bool,
    ) -> Result<()> {
        self.writer
            .authorize(
                conversation_id,
                if add {
                    ConversationMutation::TerminalRefAdd
                } else {
                    ConversationMutation::TerminalRefRemove
                },
            )
            .map(|_| ())
            .map_err(|source| repository_error("terminal_ref", conversation_id, source))
    }

    /// Add a passive reference after a PTY spawn succeeds. This never owns,
    /// attaches, detaches, or terminates the PTY.
    pub async fn add_terminal_ref(
        &self,
        conversation_id: ConversationId,
        terminal_id: &str,
    ) -> Result<SessionWorkspaceWriteOutcome> {
        self.mutate_terminal_ref(conversation_id, terminal_id, true)
            .await
    }

    /// Remove the passive reference after explicit PTY termination succeeds.
    /// View close/detach paths must never call this method.
    pub async fn remove_terminal_ref(
        &self,
        conversation_id: ConversationId,
        terminal_id: &str,
    ) -> Result<SessionWorkspaceWriteOutcome> {
        self.mutate_terminal_ref(conversation_id, terminal_id, false)
            .await
    }

    /// Remove a passive ref after the PTY is already dead. Failure is converted into an explicit
    /// recovery-required outcome and a best-effort safe recovery record; the process is never
    /// recreated.
    pub async fn remove_terminal_ref_after_termination(
        &self,
        conversation_id: ConversationId,
        terminal_id: &str,
    ) -> Result<SessionWorkspaceWriteOutcome> {
        match self.remove_terminal_ref(conversation_id, terminal_id).await {
            Ok(outcome) => Ok(outcome),
            Err(primary) => {
                let primary_code = primary.code.as_str();
                let recovery = self.record_terminal_ref_cleanup_failure(
                    conversation_id,
                    terminal_id,
                    &primary_code,
                );
                let (recovery_id, recovery_record_code) = match recovery {
                    Ok(recovery_id) => (Some(recovery_id), None),
                    Err(recovery_error) => {
                        let recovery_code = recovery_error.code.as_str();
                        log::error!(
                            "[session-workspace] terminal-ref cleanup recovery record failed conversation_id={} terminal_id={} primary_code={} recovery_code={}",
                            conversation_id,
                            terminal_id,
                            primary_code,
                            recovery_code
                        );
                        (None, Some(recovery_code))
                    }
                };
                let detail = serde_json::to_string(&json!({
                    "terminalId": terminal_id,
                    "conversationId": conversation_id,
                    "primaryCode": primary_code,
                    "recoveryId": recovery_id,
                    "recoveryRecordCode": recovery_record_code,
                }))
                .unwrap_or_else(|_| "terminal ref cleanup requires recovery".to_string());
                log::warn!(
                    "[session-workspace] terminal-ref cleanup requires recovery conversation_id={} terminal_id={} primary_code={}",
                    conversation_id,
                    terminal_id,
                    primary_code
                );
                Err(error(
                    SessionWorkspaceErrorCode::SessionWorkspaceRecoveryRequired,
                    "terminal_ref_cleanup",
                    Some(conversation_id),
                    detail,
                ))
            }
        }
    }

    /// Persist a safe recovery identity when PTY termination succeeded but passive-ref cleanup did
    /// not. The dead process is never recreated and no claim, argv, environment, or output is
    /// recorded.
    pub fn record_terminal_ref_cleanup_failure(
        &self,
        conversation_id: ConversationId,
        terminal_id: &str,
        primary_code: &str,
    ) -> Result<String> {
        self.writer
            .authorize(conversation_id, ConversationMutation::CompensationRecord)
            .map_err(|source| repository_error("terminal_ref_recovery", conversation_id, source))?;
        let relative_path = self
            .repository
            .workspace_path(conversation_id)
            .map_err(|source| repository_error("terminal_ref_recovery", conversation_id, source))?
            .strip_prefix(self.repository.root())
            .unwrap_or_else(|_| Path::new("workspace.json"))
            .to_string_lossy()
            .replace('\\', "/");
        let digest = sha256_bytes(format!("{terminal_id}\0{primary_code}").as_bytes());
        let item = RecoveryItemV1::new(
            RecoveryKind::CorruptSource,
            RecoverySeverity::Warning,
            vec![relative_path.clone()],
            vec![conversation_id],
            vec![digest.clone()],
            vec![json!({
                "reasonCode":"terminalRefCleanupFailed",
                "terminalId":terminal_id,
                "primaryCode":primary_code
            })],
            vec![RecoveryProvenanceV1 {
                source_kind: "canonical_terminal_ref".to_string(),
                relative_path,
                sha256: digest,
                preserved_read_only: true,
            }],
        );
        let recovery_id = item.recovery_id.clone();
        self.persist_recovery_item(item)?;
        Ok(recovery_id)
    }

    async fn mutate_terminal_ref(
        &self,
        conversation_id: ConversationId,
        terminal_id: &str,
        add: bool,
    ) -> Result<SessionWorkspaceWriteOutcome> {
        self.ensure_terminal_ref_writable(conversation_id, add)?;
        let lock = self.repository.workspace_lock(conversation_id);
        let _guard = lock.lock().await;
        let current_bytes = self
            .repository
            .read_workspace_bytes(conversation_id)
            .map_err(|source| repository_error("terminal_ref", conversation_id, source))?;
        let mut workspace = match current_bytes.as_deref() {
            Some(bytes) => decode_workspace(bytes, conversation_id).map_err(|reason| {
                error(
                    SessionWorkspaceErrorCode::ConversationRecoveryRequired,
                    "terminal_ref",
                    Some(conversation_id),
                    reason.reason_code(),
                )
            })?,
            None => SessionWorkspaceV1 {
                schema_version: SESSION_WORKSPACE_SCHEMA_VERSION,
                conversation_id,
                revision: 0,
                updated_at_utc: String::new(),
                update_identity: Some("host:terminalResource".to_string()),
                topology: None,
                active_pane_id: None,
                resources: Vec::new(),
                projection_state: SessionWorkspaceProjectionState::Native,
            },
        };
        let existing = workspace.resources.iter().position(|resource| {
            matches!(
                resource,
                SessionWorkspaceResourceDescriptor::Terminal {
                    terminal_id: existing,
                    ..
                } if existing == terminal_id
            )
        });
        if add && existing.is_none() {
            workspace
                .resources
                .push(SessionWorkspaceResourceDescriptor::Terminal {
                    terminal_id: terminal_id.to_string(),
                    terminal_record_id: None,
                    conversation_id,
                });
        } else if !add {
            workspace.resources.retain(|resource| {
                !matches!(
                    resource,
                    SessionWorkspaceResourceDescriptor::Terminal {
                        terminal_id: existing,
                        ..
                    } if existing == terminal_id
                )
            });
        } else {
            return Ok(SessionWorkspaceWriteOutcome::Updated {
                revision: workspace.revision,
                updated_at_utc: workspace.updated_at_utc,
            });
        }
        workspace.revision += 1;
        workspace.updated_at_utc = format_created_at_utc(&Utc::now());
        workspace.update_identity = Some("host:terminalResource".to_string());
        let mut bytes = serde_json::to_vec_pretty(&workspace).map_err(|source| {
            error(
                SessionWorkspaceErrorCode::ValidationError,
                "terminal_ref",
                Some(conversation_id),
                source.to_string(),
            )
        })?;
        bytes.push(b'\n');
        self.writer
            .replace_workspace_bytes(
                conversation_id,
                &bytes,
                if add {
                    ConversationMutation::TerminalRefAdd
                } else {
                    ConversationMutation::TerminalRefRemove
                },
            )
            .map_err(|source| repository_error("terminal_ref", conversation_id, source))?;
        log::info!(
            "[session-workspace] terminal-ref action={} conversation_id={} terminal_id={} revision={}",
            if add { "add" } else { "remove" },
            conversation_id,
            terminal_id,
            workspace.revision
        );
        Ok(SessionWorkspaceWriteOutcome::Updated {
            revision: workspace.revision,
            updated_at_utc: workspace.updated_at_utc,
        })
    }

    pub async fn load(
        &self,
        conversation_id: ConversationId,
    ) -> Result<SessionWorkspaceLoadOutcome> {
        let bytes = self
            .repository
            .read_workspace_bytes(conversation_id)
            .map_err(|source| repository_error("load", conversation_id, source))?;
        let Some(bytes) = bytes else {
            if let Some(workspace) = self.try_project_legacy_workspace(conversation_id).await? {
                log::info!(
                    "[session-workspace] legacy projection loaded conversation_id={} revision={}",
                    conversation_id,
                    workspace.revision
                );
                return Ok(SessionWorkspaceLoadOutcome::Loaded {
                    workspace: Box::new(workspace),
                });
            }
            let recovery_items = self.recovery_items_for(conversation_id)?;
            if recovery_items
                .iter()
                .any(|item| item.status == RecoveryStatus::Unresolved)
            {
                log::warn!(
                    "[session-workspace] recovery required conversation_id={} recovery_count={}",
                    conversation_id,
                    recovery_items.len()
                );
                return Ok(SessionWorkspaceLoadOutcome::RecoveryRequired {
                    conversation_id,
                    recovery_items,
                });
            }
            log::info!(
                "[session-workspace] load missing conversation_id={}",
                conversation_id
            );
            return Ok(SessionWorkspaceLoadOutcome::Missing { conversation_id });
        };
        match decode_workspace(&bytes, conversation_id) {
            Ok(workspace) => {
                log::info!(
                    "[session-workspace] load success conversation_id={} revision={} projection_state={:?}",
                    conversation_id,
                    workspace.revision,
                    workspace.projection_state
                );
                Ok(SessionWorkspaceLoadOutcome::Loaded {
                    workspace: Box::new(workspace),
                })
            }
            Err(reason) => {
                let item = self.ensure_workspace_recovery(conversation_id, &bytes, reason)?;
                log::warn!(
                    "[session-workspace] recovery required conversation_id={} recovery_id={}",
                    conversation_id,
                    item.recovery_id
                );
                Ok(SessionWorkspaceLoadOutcome::RecoveryRequired {
                    conversation_id,
                    recovery_items: vec![item],
                })
            }
        }
    }

    pub async fn write(
        &self,
        conversation_id: ConversationId,
        based_revision: Option<u64>,
        mut workspace: SessionWorkspaceV1,
    ) -> Result<SessionWorkspaceWriteOutcome> {
        validate_workspace_payload(conversation_id, &workspace)?;
        self.writer
            .authorize(conversation_id, ConversationMutation::WorkspaceWrite)
            .map_err(|source| repository_error("write", conversation_id, source))?;
        let lock = self.repository.workspace_lock(conversation_id);
        let _guard = lock.lock().await;
        let current_bytes = self
            .repository
            .read_workspace_bytes(conversation_id)
            .map_err(|source| repository_error("write", conversation_id, source))?;
        let current = match current_bytes.as_deref() {
            Some(bytes) => match decode_workspace(bytes, conversation_id) {
                Ok(workspace) => Some(workspace),
                Err(reason) => {
                    drop(_guard);
                    let item = self.ensure_workspace_recovery(conversation_id, bytes, reason)?;
                    return Ok(SessionWorkspaceWriteOutcome::RecoveryRequired {
                        recovery_items: vec![item],
                    });
                }
            },
            None => None,
        };
        let current_revision = current.as_ref().map(|value| value.revision);
        let matches = matches!(
            (based_revision, current_revision),
            (None, None) | (Some(_), Some(_))
        ) && match (based_revision, current_revision) {
            (None, None) => true,
            (Some(based), Some(existing)) => based == existing,
            _ => false,
        };
        if !matches {
            let current_revision = current.as_ref().map_or(0, |value| value.revision);
            log::warn!(
                "[session-workspace] conflict conversation_id={} current_revision={}",
                conversation_id,
                current_revision
            );
            return Ok(SessionWorkspaceWriteOutcome::Conflict {
                current_revision,
                current_updated_at_utc: current
                    .as_ref()
                    .map(|value| value.updated_at_utc.clone())
                    .unwrap_or_default(),
                current_update_identity: current.and_then(|value| value.update_identity),
            });
        }

        let revision = current_revision.unwrap_or(0) + 1;
        let updated_at_utc = format_created_at_utc(&Utc::now());
        workspace.schema_version = SESSION_WORKSPACE_SCHEMA_VERSION;
        workspace.conversation_id = conversation_id;
        workspace.revision = revision;
        workspace.updated_at_utc = updated_at_utc.clone();
        let mut bytes = serde_json::to_vec_pretty(&workspace).map_err(|source| {
            error(
                SessionWorkspaceErrorCode::ValidationError,
                "serialize",
                Some(conversation_id),
                source.to_string(),
            )
        })?;
        bytes.push(b'\n');
        self.writer
            .replace_workspace_bytes(
                conversation_id,
                &bytes,
                ConversationMutation::WorkspaceWrite,
            )
            .map_err(|source| repository_error("write", conversation_id, source))?;
        log::info!(
            "[session-workspace] write updated conversation_id={} revision={}",
            conversation_id,
            revision
        );
        Ok(SessionWorkspaceWriteOutcome::Updated {
            revision,
            updated_at_utc,
        })
    }

    /// List the durable recovery queue without mutating preserved sources or revisions.
    pub fn list_recovery_items(&self) -> Result<Vec<RecoveryItemV1>> {
        let mut items = Vec::new();
        for path in self.recovery_queue_paths()? {
            if !path.exists() {
                continue;
            }
            let bytes = fs::read(&path).map_err(|source| {
                error(
                    SessionWorkspaceErrorCode::ConversationRecoveryRequired,
                    "list_recovery_items",
                    None,
                    source.to_string(),
                )
            })?;
            let queue: RecoveryQueueV1 = serde_json::from_slice(&bytes).map_err(|source| {
                error(
                    SessionWorkspaceErrorCode::ConversationRecoveryRequired,
                    "list_recovery_items",
                    None,
                    source.to_string(),
                )
            })?;
            items.extend(queue.items);
        }
        let durable_ids = items
            .iter()
            .map(|item| item.recovery_id.clone())
            .collect::<std::collections::HashSet<_>>();
        items.extend(
            self.repository_recovery_items
                .lock()
                .iter()
                .filter(|item| !durable_ids.contains(&item.recovery_id))
                .cloned(),
        );
        items.sort_by(|left, right| left.recovery_id.cmp(&right.recovery_id));
        items.dedup_by(|left, right| left.recovery_id == right.recovery_id);
        Ok(items)
    }

    pub async fn resolve_recovery(
        &self,
        request: ResolveRecoveryItemRequest,
    ) -> Result<RecoveryActionResult> {
        let (path, mut queue) = self.find_recovery_queue(&request.recovery_id)?;
        let item = queue
            .items
            .iter()
            .find(|item| item.recovery_id == request.recovery_id)
            .cloned()
            .ok_or_else(|| {
                error(
                    SessionWorkspaceErrorCode::RecoveryNotFound,
                    "resolve_recovery",
                    None,
                    "recovery item was not found",
                )
            })?;
        let authorization = request.action.authorization();
        if authorization == crate::conversation::migration::RecoveryAuthorizationClass::Mutation {
            let targets = recovery_mutation_targets(&item, &request.action)?;
            for conversation_id in &targets {
                self.writer
                    .authorize(*conversation_id, ConversationMutation::RecoveryAction)
                    .map_err(|source| {
                        repository_error("resolve_recovery", *conversation_id, source)
                    })?;
            }
        }

        let mut workspaces = HashMap::new();
        for item in &queue.items {
            for conversation_id in &item.conversation_ids {
                if let Some(bytes) = self
                    .repository
                    .read_workspace_bytes(*conversation_id)
                    .map_err(|source| {
                        repository_error("resolve_recovery", *conversation_id, source)
                    })?
                {
                    if let Ok(workspace) = decode_workspace(&bytes, *conversation_id) {
                        workspaces.insert(*conversation_id, workspace.revision);
                    }
                }
            }
        }
        let result = queue
            .resolve(request.clone(), &mut workspaces)
            .map_err(|source| recovery_action_error(source.code, source.detail))?;

        if result.workspace_changed {
            let RecoveryAction::StartEmptyWorkspace(payload) = &request.action else {
                return Err(error(
                    SessionWorkspaceErrorCode::ConversationRecoveryRequired,
                    "resolve_recovery",
                    None,
                    "recovery result reported a workspace change for a non-workspace action",
                ));
            };
            let empty = SessionWorkspaceV1 {
                schema_version: SESSION_WORKSPACE_SCHEMA_VERSION,
                conversation_id: payload.conversation_id,
                revision: 0,
                updated_at_utc: String::new(),
                update_identity: Some("recovery:startEmptyWorkspace".to_string()),
                topology: None,
                active_pane_id: None,
                resources: Vec::new(),
                projection_state: SessionWorkspaceProjectionState::Native,
            };
            let outcome = self
                .write(
                    payload.conversation_id,
                    payload.expected_workspace_revision,
                    empty,
                )
                .await?;
            if !matches!(
                outcome,
                SessionWorkspaceWriteOutcome::Updated { revision: 1, .. }
            ) {
                return Err(error(
                    SessionWorkspaceErrorCode::ConversationConflict,
                    "resolve_recovery",
                    Some(payload.conversation_id),
                    "startEmptyWorkspace lost its absent-workspace CAS",
                ));
            }
        }
        if authorization == crate::conversation::migration::RecoveryAuthorizationClass::Mutation {
            queue
                .persist(path.parent().expect("queue path has operation directory"))
                .map_err(|source| {
                    log::error!(
                        "[session-workspace] recovery queue persistence failed recovery_id={} code=CONVERSATION_DURABILITY_FAILED",
                        request.recovery_id
                    );
                    error(
                        SessionWorkspaceErrorCode::ConversationDurabilityFailed,
                        "persist_recovery",
                        None,
                        source.to_string(),
                    )
                })?;
            self.repository_recovery_items
                .lock()
                .retain(|item| item.recovery_id != result.recovery_id);
            for conversation_id in &item.conversation_ids {
                self.writer
                    .clear_recovery_item(*conversation_id, ConversationMutation::RecoveryAction)
                    .map_err(|source| {
                        repository_error("resolve_recovery", *conversation_id, source)
                    })?;
            }
        }
        log::info!(
            "[session-workspace] recovery action={} recovery_id={} recovery_revision={} workspace_revision={:?}",
            serde_json::to_value(result.action)
                .ok()
                .and_then(|value| value.as_str().map(str::to_string))
                .unwrap_or_else(|| "unknown".to_string()),
            result.recovery_id,
            result.recovery_revision,
            result.workspace_revision
        );
        Ok(result)
    }

    async fn try_project_legacy_workspace(
        &self,
        conversation_id: ConversationId,
    ) -> Result<Option<SessionWorkspaceV1>> {
        let projection_writable = self.writer.is_writable(conversation_id);
        let Some(operation_dir) = self.migration_operation_dir()? else {
            return Ok(None);
        };
        let inventory_path = operation_dir.join(INVENTORY_FILE);
        let map_path = operation_dir.join(MIGRATION_MAP_FILE);
        let (inventory_bytes, map_bytes) = match (fs::read(&inventory_path), fs::read(&map_path)) {
            (Ok(inventory), Ok(map)) => (inventory, map),
            (Err(error), _) | (_, Err(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(None)
            }
            (Err(source), _) | (_, Err(source)) => {
                return Err(error(
                    SessionWorkspaceErrorCode::ConversationRecoveryRequired,
                    "read_projection_artifacts",
                    Some(conversation_id),
                    source.to_string(),
                ))
            }
        };
        let inventory: LegacyInventoryV1 =
            serde_json::from_slice(&inventory_bytes).map_err(|source| {
                error(
                    SessionWorkspaceErrorCode::ConversationRecoveryRequired,
                    "decode_projection_inventory",
                    Some(conversation_id),
                    source.to_string(),
                )
            })?;
        let migration_map: MigrationMapV1 =
            serde_json::from_slice(&map_bytes).map_err(|source| {
                error(
                    SessionWorkspaceErrorCode::ConversationRecoveryRequired,
                    "decode_projection_map",
                    Some(conversation_id),
                    source.to_string(),
                )
            })?;
        let by_legacy_session = migration_map
            .entries
            .iter()
            .filter_map(|entry| {
                entry
                    .legacy_agent_session_id
                    .as_ref()
                    .map(|session_id| (session_id.as_str(), entry.conversation_id))
            })
            .collect::<HashMap<_, _>>();

        struct ProjectionSource {
            bytes: Vec<u8>,
            relative_path: String,
            sha256: String,
            candidates: Vec<ConversationId>,
        }

        let mut sources = Vec::new();
        for (root_index, root) in inventory.roots.iter().enumerate() {
            if root.source_kind != LegacySourceKind::LegacyWorkspaceManifests {
                continue;
            }
            for file in &root.files {
                let path = file
                    .relative_path
                    .split('/')
                    .fold(PathBuf::from(&root.canonical_path), |path, component| {
                        path.join(component)
                    });
                let bytes = fs::read(&path).map_err(|source| {
                    error(
                        SessionWorkspaceErrorCode::ConversationRecoveryRequired,
                        "read_projection_source",
                        Some(conversation_id),
                        source.to_string(),
                    )
                })?;
                let relative_path = format!(
                    "{}/{}/{}",
                    root.source_kind.as_str(),
                    root_index,
                    file.relative_path
                );
                let actual_sha256 = sha256_bytes(&bytes);
                if actual_sha256 != file.sha256 {
                    let item = RecoveryItemV1::new(
                        RecoveryKind::CorruptSource,
                        RecoverySeverity::Blocking,
                        vec![relative_path.clone()],
                        vec![conversation_id],
                        vec![file.sha256.clone(), actual_sha256],
                        vec![json!({"reasonCode":"legacyWorkspaceSourceChanged"})],
                        vec![RecoveryProvenanceV1 {
                            source_kind: root.source_kind.as_str().to_string(),
                            relative_path,
                            sha256: file.sha256.clone(),
                            preserved_read_only: true,
                        }],
                    );
                    if projection_writable {
                        self.persist_recovery_item(item)?;
                    }
                    continue;
                }
                let value: Value = match serde_json::from_slice(&bytes) {
                    Ok(value) => value,
                    Err(_) => {
                        let item = RecoveryItemV1::new(
                            RecoveryKind::CorruptSource,
                            RecoverySeverity::Blocking,
                            vec![relative_path.clone()],
                            vec![conversation_id],
                            vec![file.sha256.clone()],
                            vec![json!({"reasonCode":"corruptLegacyWorkspaceManifest"})],
                            vec![RecoveryProvenanceV1 {
                                source_kind: root.source_kind.as_str().to_string(),
                                relative_path,
                                sha256: file.sha256.clone(),
                                preserved_read_only: true,
                            }],
                        );
                        if projection_writable {
                            self.persist_recovery_item(item)?;
                        }
                        continue;
                    }
                };
                let mut references = Vec::new();
                collect_legacy_session_references(&value, None, &mut references);
                let mut candidates = references
                    .iter()
                    .filter_map(|reference| by_legacy_session.get(reference.as_str()).copied())
                    .collect::<Vec<_>>();
                candidates.sort_by_key(ToString::to_string);
                candidates.dedup();
                if candidates.contains(&conversation_id) {
                    sources.push(ProjectionSource {
                        bytes,
                        relative_path,
                        sha256: file.sha256.clone(),
                        candidates,
                    });
                }
            }
        }
        if sources.is_empty() {
            return Ok(None);
        }
        if sources.len() > 1 {
            let item = RecoveryItemV1::new(
                RecoveryKind::AmbiguousWorkspaceManifest,
                RecoverySeverity::Warning,
                sources
                    .iter()
                    .map(|source| source.relative_path.clone())
                    .collect(),
                vec![conversation_id],
                sources.iter().map(|source| source.sha256.clone()).collect(),
                vec![json!({"reasonCode":"multipleLegacyWorkspaceSources"})],
                sources
                    .iter()
                    .map(|source| RecoveryProvenanceV1 {
                        source_kind: "legacy_workspace_manifests".to_string(),
                        relative_path: source.relative_path.clone(),
                        sha256: source.sha256.clone(),
                        preserved_read_only: true,
                    })
                    .collect(),
            );
            if projection_writable {
                self.persist_recovery_item(item)?;
            }
            return Ok(None);
        }
        let source = sources.pop().expect("one projection source remains");
        match LegacyWorkspaceProjector::project(
            &source.bytes,
            &source.relative_path,
            &source.sha256,
            conversation_id,
            &source.candidates,
            &HashMap::new(),
            Some("migration:legacyWorkspaceProjection".to_string()),
            format_created_at_utc(&Utc::now()),
        ) {
            WorkspaceProjectionOutcome::RecoveryRequired { item } => {
                if projection_writable {
                    self.persist_recovery_item(item)?;
                }
                Ok(None)
            }
            WorkspaceProjectionOutcome::Projected { workspace, .. } => {
                if !projection_writable {
                    log::info!(
                        "[session-workspace] read-only legacy projection returned without canonical mutation conversation_id={}",
                        conversation_id
                    );
                    return Ok(Some(workspace));
                }
                match self.write(conversation_id, None, workspace).await? {
                    SessionWorkspaceWriteOutcome::Updated { revision: 1, .. }
                    | SessionWorkspaceWriteOutcome::Conflict {
                        current_revision: 1,
                        ..
                    } => {
                        let bytes = self
                            .repository
                            .read_workspace_bytes(conversation_id)
                            .map_err(|source| {
                                repository_error("load_projection", conversation_id, source)
                            })?
                            .ok_or_else(|| {
                                error(
                                    SessionWorkspaceErrorCode::ConversationRecoveryRequired,
                                    "load_projection",
                                    Some(conversation_id),
                                    "projected workspace disappeared after write",
                                )
                            })?;
                        decode_workspace(&bytes, conversation_id)
                            .map(Some)
                            .map_err(|reason| {
                                error(
                                    SessionWorkspaceErrorCode::ConversationRecoveryRequired,
                                    "decode_projection",
                                    Some(conversation_id),
                                    reason.reason_code(),
                                )
                            })
                    }
                    SessionWorkspaceWriteOutcome::Conflict { .. }
                    | SessionWorkspaceWriteOutcome::RecoveryRequired { .. }
                    | SessionWorkspaceWriteOutcome::Updated { .. } => Ok(None),
                }
            }
        }
    }

    fn migration_operation_dir(&self) -> Result<Option<PathBuf>> {
        let state_root = self.state_root()?;
        let journal_path = state_root
            .join("conversation-migrations")
            .join(MIGRATION_JOURNAL_FILE);
        let bytes = match fs::read(journal_path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => {
                return Err(error(
                    SessionWorkspaceErrorCode::ConversationRecoveryRequired,
                    "read_migration_journal",
                    None,
                    source.to_string(),
                ))
            }
        };
        let journal: MigrationJournalV1 = serde_json::from_slice(&bytes).map_err(|source| {
            error(
                SessionWorkspaceErrorCode::ConversationRecoveryRequired,
                "decode_migration_journal",
                None,
                source.to_string(),
            )
        })?;
        Ok(Some(
            state_root
                .join("conversation-migrations")
                .join(journal.operation_id.to_string()),
        ))
    }

    fn recovery_items_for(&self, conversation_id: ConversationId) -> Result<Vec<RecoveryItemV1>> {
        let mut items = Vec::new();
        for path in self.recovery_queue_paths()? {
            let bytes = match fs::read(&path) {
                Ok(bytes) => bytes,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(source) => {
                    return Err(error(
                        SessionWorkspaceErrorCode::ConversationRecoveryRequired,
                        "read_recovery",
                        Some(conversation_id),
                        source.to_string(),
                    ))
                }
            };
            let queue: RecoveryQueueV1 = serde_json::from_slice(&bytes).map_err(|source| {
                error(
                    SessionWorkspaceErrorCode::ConversationRecoveryRequired,
                    "decode_recovery",
                    Some(conversation_id),
                    source.to_string(),
                )
            })?;
            items.extend(
                queue
                    .items
                    .into_iter()
                    .filter(|item| item.conversation_ids.contains(&conversation_id)),
            );
        }
        items.sort_by(|left, right| left.recovery_id.cmp(&right.recovery_id));
        items.dedup_by(|left, right| left.recovery_id == right.recovery_id);
        Ok(items)
    }

    fn ensure_workspace_recovery(
        &self,
        conversation_id: ConversationId,
        bytes: &[u8],
        reason: WorkspaceDecodeFailure,
    ) -> Result<RecoveryItemV1> {
        let relative_path = self
            .repository
            .workspace_path(conversation_id)
            .map_err(|source| repository_error("workspace_recovery", conversation_id, source))?
            .strip_prefix(self.repository.root())
            .unwrap_or_else(|_| Path::new("workspace.json"))
            .to_string_lossy()
            .replace('\\', "/");
        let sha256 = sha256_bytes(bytes);
        let item = RecoveryItemV1::new(
            RecoveryKind::CorruptSource,
            RecoverySeverity::Blocking,
            vec![relative_path.clone()],
            vec![conversation_id],
            vec![sha256.clone()],
            vec![json!({"reasonCode":reason.reason_code()})],
            vec![RecoveryProvenanceV1 {
                source_kind: "canonical_session_workspace".to_string(),
                relative_path,
                sha256,
                preserved_read_only: true,
            }],
        );
        if !self.writer.is_writable(conversation_id) {
            return Ok(item);
        }
        self.writer
            .authorize(conversation_id, ConversationMutation::RecoveryQueueWrite)
            .map_err(|source| repository_error("workspace_recovery", conversation_id, source))?;
        let path = self.primary_recovery_queue_path()?;
        let operation_dir = path.parent().expect("queue path has operation directory");
        let mut queue = match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice::<RecoveryQueueV1>(&bytes).map_err(|source| {
                error(
                    SessionWorkspaceErrorCode::ConversationRecoveryRequired,
                    "decode_recovery",
                    Some(conversation_id),
                    source.to_string(),
                )
            })?,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                RecoveryQueueV1::new(Uuid::new_v4(), Vec::new())
            }
            Err(source) => {
                return Err(error(
                    SessionWorkspaceErrorCode::ConversationRecoveryRequired,
                    "read_recovery",
                    Some(conversation_id),
                    source.to_string(),
                ))
            }
        };
        if let Some(existing) = queue
            .items
            .iter()
            .find(|existing| existing.recovery_id == item.recovery_id)
            .cloned()
        {
            return Ok(existing);
        }
        queue.items.push(item.clone());
        queue
            .items
            .sort_by(|left, right| left.recovery_id.cmp(&right.recovery_id));
        queue.persist(operation_dir).map_err(|source| {
            error(
                SessionWorkspaceErrorCode::ConversationDurabilityFailed,
                "persist_recovery",
                Some(conversation_id),
                source.to_string(),
            )
        })?;
        Ok(item)
    }

    fn persist_recovery_item(&self, item: RecoveryItemV1) -> Result<()> {
        let conversation_id = item.conversation_ids.first().copied();
        if item.conversation_ids.is_empty() {
            return Err(error(
                SessionWorkspaceErrorCode::ValidationError,
                "persist_recovery",
                None,
                "runtime recovery records require at least one candidate ConversationId",
            ));
        }
        for candidate in &item.conversation_ids {
            self.writer
                .authorize(*candidate, ConversationMutation::RecoveryQueueWrite)
                .map_err(|source| repository_error("persist_recovery", *candidate, source))?;
        }
        let path = self.primary_recovery_queue_path()?;
        let operation_dir = path.parent().expect("queue path has operation directory");
        let mut queue = match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice::<RecoveryQueueV1>(&bytes).map_err(|source| {
                error(
                    SessionWorkspaceErrorCode::ConversationRecoveryRequired,
                    "decode_recovery",
                    conversation_id,
                    source.to_string(),
                )
            })?,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                RecoveryQueueV1::new(Uuid::new_v4(), Vec::new())
            }
            Err(source) => {
                return Err(error(
                    SessionWorkspaceErrorCode::ConversationRecoveryRequired,
                    "read_recovery",
                    conversation_id,
                    source.to_string(),
                ))
            }
        };
        if queue
            .items
            .iter()
            .any(|existing| existing.recovery_id == item.recovery_id)
        {
            return Ok(());
        }
        queue.items.push(item);
        queue
            .items
            .sort_by(|left, right| left.recovery_id.cmp(&right.recovery_id));
        queue.persist(operation_dir).map_err(|source| {
            error(
                SessionWorkspaceErrorCode::ConversationDurabilityFailed,
                "persist_recovery",
                conversation_id,
                source.to_string(),
            )
        })?;
        Ok(())
    }

    fn find_recovery_queue(&self, recovery_id: &str) -> Result<(PathBuf, RecoveryQueueV1)> {
        for path in self.recovery_queue_paths()? {
            let bytes = match fs::read(&path) {
                Ok(bytes) => bytes,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(source) => {
                    return Err(error(
                        SessionWorkspaceErrorCode::ConversationRecoveryRequired,
                        "read_recovery",
                        None,
                        source.to_string(),
                    ))
                }
            };
            let queue: RecoveryQueueV1 = serde_json::from_slice(&bytes).map_err(|source| {
                error(
                    SessionWorkspaceErrorCode::ConversationRecoveryRequired,
                    "decode_recovery",
                    None,
                    source.to_string(),
                )
            })?;
            if queue
                .items
                .iter()
                .any(|item| item.recovery_id == recovery_id)
            {
                return Ok((path, queue));
            }
        }
        if let Some(item) = self
            .repository_recovery_items
            .lock()
            .iter()
            .find(|item| item.recovery_id == recovery_id)
            .cloned()
        {
            return Ok((
                self.fallback_recovery_queue_path()?,
                RecoveryQueueV1::new(Uuid::new_v4(), vec![item]),
            ));
        }
        Err(error(
            SessionWorkspaceErrorCode::RecoveryNotFound,
            "resolve_recovery",
            None,
            "recovery item was not found",
        ))
    }

    fn recovery_queue_paths(&self) -> Result<Vec<PathBuf>> {
        let mut paths = vec![self.fallback_recovery_queue_path()?];
        let state_root = self.state_root()?;
        let journal_path = state_root
            .join("conversation-migrations")
            .join(MIGRATION_JOURNAL_FILE);
        if let Ok(bytes) = fs::read(journal_path) {
            let journal: MigrationJournalV1 = serde_json::from_slice(&bytes).map_err(|source| {
                error(
                    SessionWorkspaceErrorCode::ConversationRecoveryRequired,
                    "decode_migration_journal",
                    None,
                    source.to_string(),
                )
            })?;
            paths.push(
                state_root
                    .join("conversation-migrations")
                    .join(journal.operation_id.to_string())
                    .join(RECOVERY_ITEMS_FILE),
            );
        }
        paths.sort();
        paths.dedup();
        Ok(paths)
    }

    fn primary_recovery_queue_path(&self) -> Result<PathBuf> {
        let paths = self.recovery_queue_paths()?;
        Ok(paths
            .into_iter()
            .find(|path| {
                path.exists() && !path.to_string_lossy().contains(FALLBACK_RECOVERY_OPERATION)
            })
            .unwrap_or(self.fallback_recovery_queue_path()?))
    }

    fn fallback_recovery_queue_path(&self) -> Result<PathBuf> {
        Ok(self
            .state_root()?
            .join("conversation-migrations")
            .join(FALLBACK_RECOVERY_OPERATION)
            .join(RECOVERY_ITEMS_FILE))
    }

    fn state_root(&self) -> Result<PathBuf> {
        self.repository
            .root()
            .parent()
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .ok_or_else(|| {
                error(
                    SessionWorkspaceErrorCode::SessionWorkspaceUnavailable,
                    "resolve_state_root",
                    None,
                    "canonical repository root is not <state>/conversations/v2",
                )
            })
    }
}

fn repository_recovery_items(report: &RepositoryOpenReport) -> Vec<RecoveryItemV1> {
    report
        .recovery_items
        .iter()
        .filter(|item| item.requires_action && !item.repaired)
        .map(|item| {
            let code = serde_json::to_value(item.code)
                .ok()
                .and_then(|value| value.as_str().map(str::to_string))
                .unwrap_or_else(|| "CONVERSATION_RECOVERY_REQUIRED".to_string());
            let digest = sha256_bytes(
                format!(
                    "{code}\0{:?}\0{}\0{}",
                    item.kind, item.relative_path, item.detail
                )
                .as_bytes(),
            );
            RecoveryItemV1::new(
                RecoveryKind::CorruptSource,
                RecoverySeverity::Blocking,
                vec![item.relative_path.clone()],
                item.conversation_id.into_iter().collect(),
                vec![digest.clone()],
                vec![json!({
                    "reasonCode":"repositoryRecoveryRequired",
                    "repositoryKind": serde_json::to_value(item.kind)
                        .unwrap_or_else(|_| json!("unknown")),
                    "code":code
                })],
                vec![RecoveryProvenanceV1 {
                    source_kind: "canonical_repository".to_string(),
                    relative_path: item.relative_path.clone(),
                    sha256: digest,
                    preserved_read_only: true,
                }],
            )
        })
        .collect()
}

fn recovery_mutation_targets(
    item: &RecoveryItemV1,
    action: &RecoveryAction,
) -> Result<Vec<ConversationId>> {
    let targets = match action {
        RecoveryAction::Inspect(_) => Vec::new(),
        RecoveryAction::AssociateConversation(payload) => vec![payload.conversation_id],
        RecoveryAction::StartEmptyWorkspace(payload) => vec![payload.conversation_id],
        RecoveryAction::DismissPreservedSource(_) => item.conversation_ids.clone(),
    };
    if targets.is_empty() {
        return Err(error(
            SessionWorkspaceErrorCode::ValidationError,
            "resolve_recovery",
            None,
            "mutation recovery action has no candidate ConversationId scope",
        ));
    }
    for target in &targets {
        if !item.conversation_ids.contains(target) {
            log::warn!(
                "[session-workspace] recovery candidate rejected recovery_id={} conversation_id={} code=VALIDATION_ERROR",
                item.recovery_id,
                target
            );
            return Err(error(
                SessionWorkspaceErrorCode::ValidationError,
                "resolve_recovery",
                Some(*target),
                "recovery target is outside the RecoveryItem candidate set",
            ));
        }
    }
    Ok(targets)
}

fn collect_legacy_session_references(value: &Value, key: Option<&str>, output: &mut Vec<String>) {
    match value {
        Value::Object(object) => {
            for (child_key, child) in object {
                collect_legacy_session_references(child, Some(child_key), output);
            }
        }
        Value::Array(values) => {
            for child in values {
                collect_legacy_session_references(child, key, output);
            }
        }
        Value::String(value)
            if key.is_some_and(|key| {
                let lower = key.to_ascii_lowercase();
                lower.contains("session") && lower.contains("id")
            }) =>
        {
            output.push(value.clone());
            output.sort();
            output.dedup();
        }
        _ => {}
    }
}

#[derive(Debug, Clone, Copy)]
enum WorkspaceDecodeFailure {
    CorruptJson,
    UnsupportedSchema,
    ConversationMismatch,
}

impl WorkspaceDecodeFailure {
    const fn reason_code(self) -> &'static str {
        match self {
            Self::CorruptJson => "corruptWorkspaceJson",
            Self::UnsupportedSchema => "unsupportedWorkspaceSchema",
            Self::ConversationMismatch => "workspaceConversationMismatch",
        }
    }
}

fn decode_workspace(
    bytes: &[u8],
    conversation_id: ConversationId,
) -> std::result::Result<SessionWorkspaceV1, WorkspaceDecodeFailure> {
    let value: Value =
        serde_json::from_slice(bytes).map_err(|_| WorkspaceDecodeFailure::CorruptJson)?;
    if value.get("schemaVersion").and_then(Value::as_u64)
        != Some(SESSION_WORKSPACE_SCHEMA_VERSION as u64)
    {
        return Err(WorkspaceDecodeFailure::UnsupportedSchema);
    }
    let workspace: SessionWorkspaceV1 =
        serde_json::from_value(value).map_err(|_| WorkspaceDecodeFailure::CorruptJson)?;
    if workspace.conversation_id != conversation_id {
        return Err(WorkspaceDecodeFailure::ConversationMismatch);
    }
    Ok(workspace)
}

fn validate_workspace_payload(
    conversation_id: ConversationId,
    workspace: &SessionWorkspaceV1,
) -> Result<()> {
    if workspace.schema_version != SESSION_WORKSPACE_SCHEMA_VERSION {
        return Err(error(
            SessionWorkspaceErrorCode::ValidationError,
            "validate",
            Some(conversation_id),
            "workspace schemaVersion must be 1",
        ));
    }
    if workspace.conversation_id != conversation_id {
        return Err(error(
            SessionWorkspaceErrorCode::ValidationError,
            "validate",
            Some(conversation_id),
            "request and payload ConversationId must match",
        ));
    }
    for resource in &workspace.resources {
        if let SessionWorkspaceResourceDescriptor::Terminal {
            conversation_id: resource_conversation_id,
            ..
        } = resource
        {
            if *resource_conversation_id != conversation_id {
                return Err(error(
                    SessionWorkspaceErrorCode::ValidationError,
                    "validate",
                    Some(conversation_id),
                    "terminal resource provenance must exactly match ConversationId",
                ));
            }
        }
    }
    Ok(())
}

fn repository_error(
    operation: &'static str,
    conversation_id: ConversationId,
    source: RepositoryError,
) -> SessionWorkspaceError {
    let code = match source.code {
        crate::conversation::contracts::ConversationErrorCode::ConversationNotFound => {
            SessionWorkspaceErrorCode::ConversationNotFound
        }
        crate::conversation::contracts::ConversationErrorCode::ConversationDurabilityFailed
        | crate::conversation::contracts::ConversationErrorCode::ConversationDurabilityUnsupported => {
            SessionWorkspaceErrorCode::ConversationDurabilityFailed
        }
        crate::conversation::contracts::ConversationErrorCode::LegacyCompatibilityReadOnly => {
            SessionWorkspaceErrorCode::LegacyCompatibilityReadOnly
        }
        _ => SessionWorkspaceErrorCode::ConversationRecoveryRequired,
    };
    error(code, operation, Some(conversation_id), source.to_string())
}

fn recovery_action_error(code: RecoveryActionErrorCode, detail: String) -> SessionWorkspaceError {
    let code = match code {
        RecoveryActionErrorCode::ConversationConflict => {
            SessionWorkspaceErrorCode::ConversationConflict
        }
        RecoveryActionErrorCode::MigrationIdempotencyConflict => {
            SessionWorkspaceErrorCode::MigrationIdempotencyConflict
        }
        RecoveryActionErrorCode::RecoveryNotFound => SessionWorkspaceErrorCode::RecoveryNotFound,
        RecoveryActionErrorCode::ValidationError => SessionWorkspaceErrorCode::ValidationError,
    };
    error(code, "resolve_recovery", None, detail)
}

fn error(
    code: SessionWorkspaceErrorCode,
    operation: &'static str,
    conversation_id: Option<ConversationId>,
    detail: impl Into<String>,
) -> SessionWorkspaceError {
    SessionWorkspaceError {
        code,
        operation,
        conversation_id,
        detail: detail.into(),
    }
}

fn sha256_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::contracts::{
        parse_created_at_utc, ConversationCreator, ConversationLifecycleState,
        ConversationRecordV2, CreationPartition, ExecutionTarget, CONVERSATION_SCHEMA_VERSION,
    };
    use tempfile::TempDir;

    const ID: &str = "018f7a1c-1b4d-7c8a-9f01-0123456789ab";
    const OTHER_ID: &str = "5f7a1c01-4d1b-4c8a-af01-0123456789ab";

    async fn fixture() -> (
        TempDir,
        Arc<ConversationRepository>,
        SessionWorkspaceService,
    ) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp
            .path()
            .canonicalize()
            .unwrap()
            .join("state/conversations/v2");
        let (repository, _) = ConversationRepository::open(root).unwrap();
        let writer = ConversationWriter::for_test(Arc::clone(&repository));
        for id in [ID, OTHER_ID] {
            let created_at_utc = parse_created_at_utc("2026-08-15T09:45:15.123Z").unwrap();
            writer
                .create_conversation(
                    ConversationRecordV2 {
                        schema_version: CONVERSATION_SCHEMA_VERSION,
                        conversation_id: ConversationId::parse(id).unwrap(),
                        created_at_utc,
                        creation_partition: CreationPartition::from_created_at(created_at_utc),
                        workspace_cwd: format!("/visible/{id}"),
                        execution_target: ExecutionTarget::Workspace,
                        project_attachment: None,
                        lifecycle_state: ConversationLifecycleState::Ready,
                        last_seq: 0,
                        created_by: ConversationCreator::Legacy,
                        title: None,
                        title_source: None,
                    },
                    ConversationMutation::CreateConversation,
                )
                .await
                .unwrap();
        }
        let service = SessionWorkspaceService::new(writer);
        (temp, repository, service)
    }

    fn workspace(id: &str, identity: &str) -> SessionWorkspaceV1 {
        let conversation_id = ConversationId::parse(id).unwrap();
        SessionWorkspaceV1 {
            schema_version: SESSION_WORKSPACE_SCHEMA_VERSION,
            conversation_id,
            revision: 999,
            updated_at_utc: "ignored".to_string(),
            update_identity: Some(identity.to_string()),
            topology: Some(SessionWorkspacePaneNode::Leaf(SessionWorkspaceLeafNode {
                id: "leaf-one".to_string(),
                terminal_ids: Vec::new(),
                editor_ids: vec!["edit-one".to_string()],
                active_tab_id: Some("edit-one".to_string()),
            })),
            active_pane_id: Some("leaf-one".to_string()),
            resources: vec![SessionWorkspaceResourceDescriptor::Editor {
                editor_id: "edit-one".to_string(),
                file_path: "/src/main.rs".to_string(),
            }],
            projection_state: SessionWorkspaceProjectionState::Native,
        }
    }

    #[tokio::test]
    async fn initial_update_conflict_serialization_and_durable_reopen() {
        let (_temp, repository, service) = fixture().await;
        let id = ConversationId::parse(ID).unwrap();
        assert!(matches!(
            service.load(id).await.unwrap(),
            SessionWorkspaceLoadOutcome::Missing { .. }
        ));
        assert!(matches!(
            service.write(id, None, workspace(ID, "one")).await.unwrap(),
            SessionWorkspaceWriteOutcome::Updated { revision: 1, .. }
        ));
        assert!(matches!(
            service
                .write(id, Some(1), workspace(ID, "two"))
                .await
                .unwrap(),
            SessionWorkspaceWriteOutcome::Updated { revision: 2, .. }
        ));
        let path = repository.workspace_path(id).unwrap();
        let before = fs::read(&path).unwrap();
        let conflict = service
            .write(id, Some(1), workspace(ID, "stale"))
            .await
            .unwrap();
        assert!(matches!(
            conflict,
            SessionWorkspaceWriteOutcome::Conflict {
                current_revision: 2,
                ..
            }
        ));
        assert_eq!(fs::read(&path).unwrap(), before);

        let (reopened, _) = ConversationRepository::open(repository.root().to_path_buf()).unwrap();
        let reopened_writer = ConversationWriter::for_test(reopened);
        let reopened_service = SessionWorkspaceService::new(reopened_writer);
        let SessionWorkspaceLoadOutcome::Loaded {
            workspace: loaded_workspace,
        } = reopened_service.load(id).await.unwrap()
        else {
            panic!("expected persisted workspace");
        };
        assert_eq!(loaded_workspace.revision, 2);
        assert_eq!(loaded_workspace.update_identity.as_deref(), Some("two"));

        let first = Arc::new(service);
        let a = Arc::clone(&first);
        let b = Arc::clone(&first);
        let (left, right) = tokio::join!(
            async move { a.write(id, Some(2), workspace(ID, "a")).await.unwrap() },
            async move { b.write(id, Some(2), workspace(ID, "b")).await.unwrap() }
        );
        let outcomes = [left, right];
        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| matches!(
                    outcome,
                    SessionWorkspaceWriteOutcome::Updated { revision: 3, .. }
                ))
                .count(),
            1
        );
        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| matches!(
                    outcome,
                    SessionWorkspaceWriteOutcome::Conflict {
                        current_revision: 3,
                        ..
                    }
                ))
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn conversations_are_isolated_and_payload_identity_is_forced_to_match() {
        let (_temp, _repository, service) = fixture().await;
        let one = ConversationId::parse(ID).unwrap();
        let two = ConversationId::parse(OTHER_ID).unwrap();
        service
            .write(one, None, workspace(ID, "one"))
            .await
            .unwrap();
        service
            .write(two, None, workspace(OTHER_ID, "two"))
            .await
            .unwrap();
        let SessionWorkspaceLoadOutcome::Loaded {
            workspace: loaded_one,
        } = service.load(one).await.unwrap()
        else {
            panic!("workspace one");
        };
        let SessionWorkspaceLoadOutcome::Loaded {
            workspace: loaded_two,
        } = service.load(two).await.unwrap()
        else {
            panic!("workspace two");
        };
        assert_eq!(loaded_one.update_identity.as_deref(), Some("one"));
        assert_eq!(loaded_two.update_identity.as_deref(), Some("two"));
        let mismatch = service
            .write(one, Some(1), workspace(OTHER_ID, "bad"))
            .await;
        assert_eq!(
            mismatch.unwrap_err().code,
            SessionWorkspaceErrorCode::ValidationError
        );
    }

    #[tokio::test]
    async fn unsupported_schema_and_corrupt_json_fail_closed_into_durable_recovery() {
        let (_temp, repository, service) = fixture().await;
        let id = ConversationId::parse(ID).unwrap();
        let path = repository.workspace_path(id).unwrap();
        fs::write(
            &path,
            format!(r#"{{"schemaVersion":99,"conversationId":"{ID}"}}"#),
        )
        .unwrap();
        let SessionWorkspaceLoadOutcome::RecoveryRequired { recovery_items, .. } =
            service.load(id).await.unwrap()
        else {
            panic!("unsupported schema must require recovery");
        };
        assert_eq!(recovery_items.len(), 1);
        let queue_path = service.fallback_recovery_queue_path().unwrap();
        assert!(queue_path.is_file());
        let unsupported_bytes = fs::read(&path).unwrap();

        fs::write(&path, b"not-json").unwrap();
        assert!(matches!(
            service.load(id).await.unwrap(),
            SessionWorkspaceLoadOutcome::RecoveryRequired { .. }
        ));
        assert_eq!(
            unsupported_bytes,
            format!(r#"{{"schemaVersion":99,"conversationId":"{ID}"}}"#).as_bytes()
        );
        assert_eq!(fs::read(&path).unwrap(), b"not-json");
        let reopened_queue: RecoveryQueueV1 =
            serde_json::from_slice(&fs::read(queue_path).unwrap()).unwrap();
        assert_eq!(reopened_queue.items.len(), 1);
        assert_eq!(reopened_queue.items[0].source_sha256.len(), 1);

        let (reopened_repository, report) =
            ConversationRepository::open(repository.root().to_path_buf()).unwrap();
        assert!(report.recovery_items.iter().any(|item| {
            item.kind
                == crate::conversation::repository::RepositoryRecoveryKind::WorkspaceRecoveryRequired
        }));
        let reopened_writer = ConversationWriter::for_test(reopened_repository);
        let reopened_service = SessionWorkspaceService::new(reopened_writer);
        assert!(matches!(
            reopened_service.load(id).await.unwrap(),
            SessionWorkspaceLoadOutcome::RecoveryRequired { .. }
        ));
    }

    #[tokio::test]
    async fn missing_workspace_projects_one_uniquely_attributable_legacy_manifest() {
        use crate::acp::workspace_manifest::{
            EditorDescriptor, LeafNode, PaneNode, TerminalDescriptor, WorkspaceManifest,
            WorkspaceManifestFile,
        };
        use crate::conversation::migration::{
            CreatedAtSource, IdentityDecision, LegacyInventoryFileV1, LegacyInventoryRootV1,
            LegacySourceKind, MigrationJournalV1, MigrationMapEntryV1, MigrationMapV1,
            LEGACY_INVENTORY_SCHEMA_VERSION, MIGRATION_MAP_SCHEMA_VERSION,
        };

        let (_temp, repository, service) = fixture().await;
        let conversation_id = ConversationId::parse(ID).unwrap();
        let state_root = repository
            .root()
            .parent()
            .and_then(Path::parent)
            .unwrap()
            .to_path_buf();
        let legacy_root = state_root.join("workspace-manifests");
        fs::create_dir_all(&legacy_root).unwrap();
        let source_path = legacy_root.join("project-one.json");
        let source_bytes = serde_json::to_vec(&WorkspaceManifestFile {
            schema_version: 1,
            manifest: WorkspaceManifest {
                project_id: "project-one".to_string(),
                revision: 4,
                update_identity: None,
                updated_at: 1,
                topology: Some(PaneNode::Leaf(LeafNode {
                    id: "leaf-projected".to_string(),
                    terminal_ids: vec!["terminal-unowned".to_string()],
                    editor_ids: vec!["editor-one".to_string()],
                    active_tab_id: Some("editor-one".to_string()),
                })),
                active_pane_id: Some("leaf-projected".to_string()),
                focused_session_id: Some("opaque-session-one".to_string()),
                terminals: vec![TerminalDescriptor {
                    terminal_id: "terminal-unowned".to_string(),
                    project_id: "project-one".to_string(),
                    shell: "bash".to_string(),
                    cwd: "/legacy/project".to_string(),
                    name: "legacy".to_string(),
                    worktree_id: None,
                    claim_handle: None,
                }],
                editors: vec![EditorDescriptor {
                    editor_id: "editor-one".to_string(),
                    file_path: "/legacy/project/src/main.ts".to_string(),
                }],
            },
        })
        .unwrap();
        fs::write(&source_path, &source_bytes).unwrap();
        let source_sha256 = sha256_bytes(&source_bytes);

        let migration_dir = state_root.join("conversation-migrations");
        fs::create_dir_all(&migration_dir).unwrap();
        let journal = MigrationJournalV1::new("a".repeat(64), Utc::now());
        let operation_dir = migration_dir.join(journal.operation_id.to_string());
        fs::create_dir_all(&operation_dir).unwrap();
        fs::write(
            migration_dir.join(MIGRATION_JOURNAL_FILE),
            serde_json::to_vec(&journal).unwrap(),
        )
        .unwrap();
        let inventory = LegacyInventoryV1 {
            schema_version: LEGACY_INVENTORY_SCHEMA_VERSION,
            operation_id: journal.operation_id,
            generated_at_utc: format_created_at_utc(&Utc::now()),
            roots: vec![LegacyInventoryRootV1 {
                source_kind: LegacySourceKind::LegacyWorkspaceManifests,
                canonical_path: legacy_root.to_string_lossy().into_owned(),
                files: vec![LegacyInventoryFileV1 {
                    relative_path: "project-one.json".to_string(),
                    size: source_bytes.len() as u64,
                    sha256: source_sha256.clone(),
                }],
            }],
            inventory_sha256: "b".repeat(64),
        };
        fs::write(
            operation_dir.join(INVENTORY_FILE),
            serde_json::to_vec(&inventory).unwrap(),
        )
        .unwrap();
        let migration_map = MigrationMapV1 {
            schema_version: MIGRATION_MAP_SCHEMA_VERSION,
            operation_id: journal.operation_id,
            entries: vec![MigrationMapEntryV1 {
                source_key: "legacy_host_session:0:one".to_string(),
                legacy_storage_key: Some(ID.to_string()),
                legacy_agent_session_id: Some("opaque-session-one".to_string()),
                conversation_id,
                identity_decision: IdentityDecision::ReusedUniqueStorageUuid,
                created_at_source: Some(CreatedAtSource::HostMetadata),
                source_record_sha256: "c".repeat(64),
            }],
        };
        fs::write(
            operation_dir.join(MIGRATION_MAP_FILE),
            serde_json::to_vec(&migration_map).unwrap(),
        )
        .unwrap();

        let SessionWorkspaceLoadOutcome::Loaded { workspace } =
            service.load(conversation_id).await.unwrap()
        else {
            panic!("unique legacy workspace must project");
        };
        assert_eq!(workspace.revision, 1);
        assert!(matches!(
            workspace.projection_state,
            SessionWorkspaceProjectionState::Projected {
                projected_resource_count: 1,
                unresolved_resource_count: 1,
                ..
            }
        ));
        assert!(workspace.resources.iter().all(|resource| !matches!(
            resource,
            SessionWorkspaceResourceDescriptor::Terminal { .. }
        )));
        assert_eq!(fs::read(source_path).unwrap(), source_bytes);
        assert_eq!(
            sha256_bytes(&fs::read(legacy_root.join("project-one.json")).unwrap()),
            source_sha256
        );
    }

    #[tokio::test]
    async fn passive_terminal_refs_survive_view_close_and_change_only_on_spawn_terminate() {
        let (_temp, _repository, service) = fixture().await;
        let id = ConversationId::parse(ID).unwrap();
        let added = service.add_terminal_ref(id, "pty-one").await.unwrap();
        assert!(matches!(
            added,
            SessionWorkspaceWriteOutcome::Updated { revision: 1, .. }
        ));
        let SessionWorkspaceLoadOutcome::Loaded { workspace } = service.load(id).await.unwrap()
        else {
            panic!("workspace loaded");
        };
        assert!(workspace.resources.iter().any(|resource| matches!(
            resource,
            SessionWorkspaceResourceDescriptor::Terminal { terminal_id, .. }
                if terminal_id == "pty-one"
        )));

        let removed = service.remove_terminal_ref(id, "pty-one").await.unwrap();
        assert!(matches!(
            removed,
            SessionWorkspaceWriteOutcome::Updated { revision: 2, .. }
        ));
        let SessionWorkspaceLoadOutcome::Loaded { workspace } = service.load(id).await.unwrap()
        else {
            panic!("workspace loaded");
        };
        assert!(!workspace.resources.iter().any(|resource| matches!(
            resource,
            SessionWorkspaceResourceDescriptor::Terminal { terminal_id, .. }
                if terminal_id == "pty-one"
        )));
    }

    #[tokio::test]
    async fn dead_terminal_ref_cleanup_failure_records_recovery_without_process_recreation() {
        let (_temp, repository, service) = fixture().await;
        let id = ConversationId::parse(ID).unwrap();
        service.add_terminal_ref(id, "pty-dead").await.unwrap();
        repository.fail_next_workspace_replace();

        let error = service
            .remove_terminal_ref_after_termination(id, "pty-dead")
            .await
            .unwrap_err();
        assert_eq!(
            error.code,
            SessionWorkspaceErrorCode::SessionWorkspaceRecoveryRequired
        );
        let detail: Value = serde_json::from_str(&error.detail).unwrap();
        assert_eq!(detail["terminalId"], "pty-dead");
        assert_eq!(detail["conversationId"], ID);
        assert_eq!(detail["primaryCode"], "CONVERSATION_DURABILITY_FAILED");
        assert!(detail["recoveryId"].as_str().is_some());
        assert!(service
            .list_recovery_items()
            .unwrap()
            .iter()
            .any(|item| item.status == RecoveryStatus::Unresolved));
        let SessionWorkspaceLoadOutcome::Loaded { workspace } = service.load(id).await.unwrap()
        else {
            panic!("workspace remains available for recovery")
        };
        assert!(workspace.resources.iter().any(|resource| matches!(
            resource,
            SessionWorkspaceResourceDescriptor::Terminal { terminal_id, .. }
                if terminal_id == "pty-dead"
        )));
    }

    #[test]
    fn raw_claim_env_and_credentials_are_rejected_by_descriptors() {
        for field in ["claim", "envVars", "credentials"] {
            let mut value = json!({
                "kind":"terminal",
                "terminalId":"terminal-one",
                "conversationId":ID
            });
            value
                .as_object_mut()
                .unwrap()
                .insert(field.to_string(), json!("secret"));
            assert!(serde_json::from_value::<SessionWorkspaceResourceDescriptor>(value).is_err());
        }
    }
}
