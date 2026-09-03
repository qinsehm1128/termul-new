//! Read-only legacy and hybrid Conversation projection.
//!
//! Compatibility opens preserved source artifacts through read-only OS handles and exposes no
//! mutation methods. Every admitted write target is an existing ConversationRepository, and
//! hybrid rollback rejects mutation of mapped legacy conversations.

use std::collections::{HashMap, HashSet};
#[cfg(any(test, not(unix)))]
use std::fs;
use std::fs::{File, OpenOptions};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{TimeZone, Utc};

use crate::conversation::contracts::{
    ConversationCreator, ConversationId, ConversationLifecycleState, ConversationRecordV2,
    CreationPartition, ExecutionTarget, CONVERSATION_SCHEMA_VERSION,
};
use crate::conversation::migration::layout::ReaderPrecedence;
use crate::conversation::migration::legacy::{CreatedAtSource, MigrationMapV1};
use crate::conversation::repository::ConversationRepository;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyConversationProjection {
    pub record: ConversationRecordV2,
    pub source_key: String,
    pub opaque_agent_session_id: Option<String>,
    pub source_record_sha256: String,
}

#[derive(Debug)]
pub struct CompatibilityError {
    pub code: &'static str,
    pub detail: String,
}

impl std::fmt::Display for CompatibilityError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.code, self.detail)
    }
}

impl std::error::Error for CompatibilityError {}

#[derive(Debug, Clone, Default)]
pub struct LegacyConversationReader {
    projections: HashMap<ConversationId, LegacyConversationProjection>,
    mapped_ids: HashSet<ConversationId>,
    source_handles: Vec<Arc<File>>,
}

impl LegacyConversationReader {
    /// Build the legacy projection from durable mappings and source metadata using read-only
    /// handles. Invalid/blocking records are absent rather than guessed from filesystem time.
    pub fn open_read_only(
        map: &MigrationMapV1,
        source_roots: &[PathBuf],
    ) -> Result<Self, CompatibilityError> {
        let mut source_handles = Vec::new();
        for root in source_roots {
            if !root.exists() {
                continue;
            }
            source_handles.push(Arc::new(open_directory_or_marker_read_only(root)?));
        }
        let mut projections = HashMap::new();
        let mut mapped_ids = HashSet::new();
        for entry in &map.entries {
            mapped_ids.insert(entry.conversation_id);
            let Some(created_at_source) = entry.created_at_source else {
                continue;
            };
            let Some(metadata_path) = source_metadata_path(&entry.source_key, source_roots) else {
                continue;
            };
            let mut file = OpenOptions::new()
                .read(true)
                .open(&metadata_path)
                .map_err(|error| CompatibilityError {
                    code: "LEGACY_COMPATIBILITY_READ_FAILED",
                    detail: format!("preserved metadata could not be opened read-only: {error}"),
                })?;
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes)
                .map_err(|error| CompatibilityError {
                    code: "LEGACY_COMPATIBILITY_READ_FAILED",
                    detail: error.to_string(),
                })?;
            let value: serde_json::Value =
                serde_json::from_slice(&bytes).map_err(|error| CompatibilityError {
                    code: "LEGACY_COMPATIBILITY_READ_FAILED",
                    detail: error.to_string(),
                })?;
            let metadata = if created_at_source == CreatedAtSource::HostMetadata {
                &value
            } else {
                value
                    .get("payload")
                    .and_then(|payload| payload.get("metadata"))
                    .unwrap_or(&value)
            };
            let Some(created_at_millis) =
                metadata.get("createdAt").and_then(|value| value.as_i64())
            else {
                continue;
            };
            let Some(created_at_utc) = Utc.timestamp_millis_opt(created_at_millis).single() else {
                continue;
            };
            let Some(workspace_cwd) = metadata
                .get("cwd")
                .and_then(|value| value.as_str())
                .filter(|value| !value.is_empty())
                .map(str::to_string)
            else {
                continue;
            };
            projections.insert(
                entry.conversation_id,
                LegacyConversationProjection {
                    record: ConversationRecordV2 {
                        schema_version: CONVERSATION_SCHEMA_VERSION,
                        conversation_id: entry.conversation_id,
                        created_at_utc,
                        creation_partition: CreationPartition::from_created_at(created_at_utc),
                        workspace_cwd,
                        execution_target: ExecutionTarget::Workspace,
                        project_attachment: None,
                        lifecycle_state: ConversationLifecycleState::Ready,
                        last_seq: 0,
                        // The projection is a record *this* build emits now, so
                        // it claims this build's creator (decision OD-04). The
                        // fact that the underlying data predates the rename is
                        // carried by `source_key` / `source_record_sha256`, not
                        // by `created_by`.
                        created_by: ConversationCreator::SeManager,
                        title: None,
                        title_source: None,
                    },
                    source_key: entry.source_key.clone(),
                    opaque_agent_session_id: entry.legacy_agent_session_id.clone(),
                    source_record_sha256: entry.source_record_sha256.clone(),
                },
            );
        }
        Ok(Self {
            projections,
            mapped_ids,
            source_handles,
        })
    }

    #[cfg(test)]
    fn from_projections(projections: Vec<LegacyConversationProjection>) -> Self {
        let mapped_ids = projections
            .iter()
            .map(|projection| projection.record.conversation_id)
            .collect();
        Self {
            projections: projections
                .into_iter()
                .map(|projection| (projection.record.conversation_id, projection))
                .collect(),
            mapped_ids,
            source_handles: Vec::new(),
        }
    }

    #[must_use]
    pub fn get(&self, conversation_id: ConversationId) -> Option<LegacyConversationProjection> {
        self.projections.get(&conversation_id).cloned()
    }

    #[must_use]
    pub fn list(&self) -> Vec<LegacyConversationProjection> {
        let mut projections = self.projections.values().cloned().collect::<Vec<_>>();
        projections.sort_by_key(|projection| projection.record.conversation_id.to_string());
        projections
    }

    #[must_use]
    pub fn is_mapped(&self, conversation_id: ConversationId) -> bool {
        self.mapped_ids.contains(&conversation_id)
    }

    #[must_use]
    pub fn open_handle_count(&self) -> usize {
        self.source_handles.len()
    }
}

pub struct ConversationReader {
    repository: Arc<ConversationRepository>,
    legacy: LegacyConversationReader,
    precedence: ReaderPrecedence,
}

impl ConversationReader {
    #[must_use]
    pub fn new(
        repository: Arc<ConversationRepository>,
        legacy: LegacyConversationReader,
        precedence: ReaderPrecedence,
    ) -> Self {
        Self {
            repository,
            legacy,
            precedence,
        }
    }

    pub fn get(
        &self,
        conversation_id: ConversationId,
    ) -> Result<ConversationRecordV2, CompatibilityError> {
        match self.precedence {
            ReaderPrecedence::LegacyOnly => self
                .legacy
                .get(conversation_id)
                .map(|projection| projection.record)
                .ok_or_else(not_found),
            ReaderPrecedence::ConversationV2First => self
                .repository
                .get_conversation(conversation_id)
                .or_else(|_| {
                    self.legacy
                        .get(conversation_id)
                        .map(|projection| projection.record)
                        .ok_or_else(not_found_repository)
                })
                .map_err(repository_compatibility_error),
            ReaderPrecedence::HybridLegacyFirst => {
                if let Some(projection) = self.legacy.get(conversation_id) {
                    Ok(projection.record)
                } else {
                    self.repository
                        .get_conversation(conversation_id)
                        .map_err(repository_compatibility_error)
                }
            }
            ReaderPrecedence::ConversationV2Only => self
                .repository
                .get_conversation(conversation_id)
                .map_err(repository_compatibility_error),
        }
    }

    #[must_use]
    pub fn list(&self) -> Vec<ConversationRecordV2> {
        let mut records = HashMap::new();
        match self.precedence {
            ReaderPrecedence::LegacyOnly => {
                for projection in self.legacy.list() {
                    records.insert(projection.record.conversation_id, projection.record);
                }
            }
            ReaderPrecedence::ConversationV2First => {
                for projection in self.legacy.list() {
                    records.insert(projection.record.conversation_id, projection.record);
                }
                for record in self.repository.list_conversations() {
                    records.insert(record.conversation_id, record);
                }
            }
            ReaderPrecedence::HybridLegacyFirst => {
                for record in self.repository.list_conversations() {
                    records.insert(record.conversation_id, record);
                }
                for projection in self.legacy.list() {
                    records.insert(projection.record.conversation_id, projection.record);
                }
            }
            ReaderPrecedence::ConversationV2Only => {
                for record in self.repository.list_conversations() {
                    records.insert(record.conversation_id, record);
                }
            }
        }
        let mut records = records.into_values().collect::<Vec<_>>();
        records.retain(|record| record.lifecycle_state != ConversationLifecycleState::Deleted);
        records.sort_by_key(|record| record.conversation_id.to_string());
        records
    }
}

fn source_metadata_path(source_key: &str, roots: &[PathBuf]) -> Option<PathBuf> {
    let mut parts = source_key.splitn(3, ':');
    let kind = parts.next()?;
    let root_index = parts.next()?.parse::<usize>().ok()?;
    let relative = parts.next()?;
    let root = roots.get(root_index)?;
    match kind {
        "legacy_host_session" => Some(root.join(relative).join("metadata.json")),
        "legacy_chat_history" => Some(root.join(relative)),
        _ => None,
    }
}

#[cfg(unix)]
fn open_directory_or_marker_read_only(path: &Path) -> Result<File, CompatibilityError> {
    File::open(path).map_err(|error| CompatibilityError {
        code: "LEGACY_COMPATIBILITY_READ_FAILED",
        detail: error.to_string(),
    })
}

#[cfg(not(unix))]
fn open_directory_or_marker_read_only(path: &Path) -> Result<File, CompatibilityError> {
    let marker = fs::read_dir(path)
        .map_err(|error| CompatibilityError {
            code: "LEGACY_COMPATIBILITY_READ_FAILED",
            detail: error.to_string(),
        })?
        .flatten()
        .map(|entry| entry.path())
        .find(|entry| entry.is_file())
        .ok_or_else(|| CompatibilityError {
            code: "LEGACY_COMPATIBILITY_READ_FAILED",
            detail: "legacy root has no readable marker file".to_string(),
        })?;
    File::open(marker).map_err(|error| CompatibilityError {
        code: "LEGACY_COMPATIBILITY_READ_FAILED",
        detail: error.to_string(),
    })
}

fn not_found() -> CompatibilityError {
    CompatibilityError {
        code: "CONVERSATION_NOT_FOUND",
        detail: "Conversation was not found in the active reader layout".to_string(),
    }
}

fn not_found_repository() -> crate::conversation::repository::RepositoryError {
    crate::conversation::repository::RepositoryError {
        code: crate::conversation::contracts::ConversationErrorCode::ConversationNotFound,
        operation: "compatibility_read",
        conversation_id: None,
        detail: "Conversation was not found in the active reader layout".to_string(),
    }
}

fn repository_compatibility_error(
    error: crate::conversation::repository::RepositoryError,
) -> CompatibilityError {
    let code = match error.code {
        crate::conversation::contracts::ConversationErrorCode::ConversationNotFound => {
            "CONVERSATION_NOT_FOUND"
        }
        _ => "CONVERSATION_READ_FAILED",
    };
    CompatibilityError {
        code,
        detail: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::contracts::{
        parse_created_at_utc, ConversationId, ConversationRecordV2,
    };

    const LEGACY_ID: &str = "018f7a1c-1b4d-7c8a-9f01-0123456789ab";
    const NEW_ID: &str = "5f7a1c01-4d1b-4c8a-af01-0123456789ab";

    fn record(id: &str, cwd: &str) -> ConversationRecordV2 {
        let created_at_utc = parse_created_at_utc("2026-08-15T09:45:15.123Z").unwrap();
        ConversationRecordV2 {
            schema_version: CONVERSATION_SCHEMA_VERSION,
            conversation_id: ConversationId::parse(id).unwrap(),
            created_at_utc,
            creation_partition: CreationPartition::from_created_at(created_at_utc),
            workspace_cwd: cwd.to_string(),
            execution_target: ExecutionTarget::Workspace,
            project_attachment: None,
            lifecycle_state: ConversationLifecycleState::Ready,
            last_seq: 0,
            created_by: ConversationCreator::Legacy,
            title: None,
            title_source: None,
        }
    }

    #[tokio::test]
    async fn hybrid_rollback_lists_legacy_projection_and_new_v2_only_conversation() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap().join("v2");
        let (repository, _) = ConversationRepository::open(root).unwrap();
        let seed_writer =
            crate::conversation::ConversationWriter::for_test(Arc::clone(&repository));
        let legacy_record = record(LEGACY_ID, "/legacy");
        seed_writer
            .create_conversation(
                legacy_record.clone(),
                crate::conversation::ConversationMutation::CreateConversation,
            )
            .await
            .unwrap();
        let new_record = record(NEW_ID, "/new-project-less");
        seed_writer
            .create_conversation(
                new_record.clone(),
                crate::conversation::ConversationMutation::CreateConversation,
            )
            .await
            .unwrap();
        let legacy =
            LegacyConversationReader::from_projections(vec![LegacyConversationProjection {
                record: legacy_record.clone(),
                source_key: "legacy_host_session:0:legacy".to_string(),
                opaque_agent_session_id: Some("provider/session:abc".to_string()),
                source_record_sha256: "a".repeat(64),
            }]);
        let reader = ConversationReader::new(
            Arc::clone(&repository),
            legacy,
            ReaderPrecedence::HybridLegacyFirst,
        );
        let listed = reader.list();
        assert_eq!(listed.len(), 2);
        assert_eq!(
            reader.get(legacy_record.conversation_id).unwrap(),
            legacy_record
        );
        assert_eq!(reader.get(new_record.conversation_id).unwrap(), new_record);
        let authority = Arc::new(crate::conversation::ConversationWriteAuthority::new(
            repository.as_ref(),
            ReaderPrecedence::HybridLegacyFirst,
            [ConversationId::parse(LEGACY_ID).unwrap()],
        ));
        let writer =
            crate::conversation::ConversationWriter::new(Arc::clone(&repository), authority)
                .unwrap();
        let error = writer
            .authorize(
                ConversationId::parse(LEGACY_ID).unwrap(),
                crate::conversation::ConversationMutation::MetadataUpdate,
            )
            .unwrap_err();
        assert_eq!(
            error.code,
            crate::conversation::ConversationErrorCode::LegacyCompatibilityReadOnly
        );
        assert!(writer
            .authorize(
                ConversationId::parse(NEW_ID).unwrap(),
                crate::conversation::ConversationMutation::MetadataUpdate,
            )
            .is_ok());
        assert_eq!(repository.list_conversations().len(), 2);
    }

    #[test]
    fn legacy_reader_holds_only_read_handles() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("legacy");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("marker.json"), b"{}").unwrap();
        let reader = LegacyConversationReader::open_read_only(
            &MigrationMapV1 {
                schema_version: 1,
                operation_id: uuid::Uuid::new_v4(),
                entries: Vec::new(),
            },
            &[root],
        )
        .unwrap();
        assert_eq!(reader.open_handle_count(), 1);
    }
}
