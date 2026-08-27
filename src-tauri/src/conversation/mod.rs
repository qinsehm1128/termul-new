//! Host-owned Conversation domain contracts and services.

pub mod application;
pub mod bootstrap;
pub mod catalog;
pub mod contracts;
pub mod creation;
pub mod durable_fs;
pub mod event_log;
pub mod lifecycle;
pub mod locator;
pub mod migration;
pub mod ordered_persistence;
pub mod persistence_adapter;
pub mod repository;
pub mod session_workspace;
pub mod usage_plan;
pub mod workspace_projection;
pub mod write_authority;

#[cfg(test)]
mod native_durability_tests;
#[cfg(test)]
mod validation_tests;

pub use application::{
    ConversationAggregateMutationAction, ConversationAggregateMutationOutcome,
    ConversationApplicationError, ConversationApplicationService, ConversationBindingSnapshot,
    ConversationHostKind, ConversationHostState, ConversationHostStatus,
    ConversationIdentitySnapshot, ConversationOpenOutcome, LegacyConversationKey,
    LegacyConversationResolution, LegacyConversationSourceKind,
};
pub use bootstrap::{
    BootstrapError, BootstrapOutcome, ConversationBootstrap, HostConversationRoots,
};
pub use catalog::{
    rebuild_catalog, AcceptedCanonicalConversation, CatalogAdmissionMetrics, CatalogError,
    CatalogRebuildResult, CatalogRecoveryIssue, CatalogReplaceAck, CatalogReplaceFence,
    ConversationCatalogEntryV1, ConversationCatalogFileV1, ConversationCatalogGeneration,
    ConversationCatalogSnapshot, ConversationProvenanceFileV1, ConversationProvenanceSourceV1,
    CATALOG_CHUNK_ENTRIES, CATALOG_FILE, CATALOG_SCHEMA_VERSION, CONVERSATION_METADATA_FILE,
    EMPTY_CATALOG_GENERATED_AT_UTC, PROVENANCE_FILE, PROVENANCE_SCHEMA_VERSION,
};
pub use contracts::{
    format_created_at_utc, parse_created_at_utc, AgentSessionBinding, AgentSessionBindingState,
    ConversationCreator, ConversationErrorCode, ConversationHistoryPageV1,
    ConversationHistoryPageValidationError, ConversationHistoryRecordV1, ConversationId,
    ConversationIdPathError, ConversationLifecycleState, ConversationRecordV2,
    ConversationTitleSource, CreatedAtUtcError, CreationPartition, ExecutionTarget,
    ProjectAttachment, TerminalResourceRef, AGENT_SESSION_BINDING_SCHEMA_VERSION,
    CONVERSATION_HISTORY_PAGE_SCHEMA_VERSION, CONVERSATION_HISTORY_RECORD_SCHEMA_VERSION,
    CONVERSATION_SCHEMA_VERSION, MAX_CONVERSATION_HISTORY_PAGE_BYTES,
    MAX_CONVERSATION_HISTORY_PAGE_LIMIT, MAX_CONVERSATION_RECORD_BYTES,
    MIN_CONVERSATION_HISTORY_PAGE_LIMIT, PROJECT_ATTACHMENT_SCHEMA_VERSION,
    TERMINAL_RESOURCE_REF_SCHEMA_VERSION,
};
pub use creation::{
    AgentBindingResult, AgentCompensationFailure, AgentCreationFailure, Clock,
    ConversationCreationError, ConversationCreationService, ConversationIdGenerator,
    DefaultConversationIdGenerator, PrepareConversationRequest, PreparedConversation, SystemClock,
    ACP_COMPENSATION_FAILED, PREPARED_CONVERSATION_SCHEMA_VERSION,
    PREPARE_CONVERSATION_SCHEMA_VERSION,
};
pub use durable_fs::{
    append_jsonl, create_dir_durable, replace_bytes, sync_file_and_namespace, CrashInjector,
    CrashPoint, DirectoryPermissions, DurabilityLevel, DurableFileSystem, DurableFsError,
    DurableWriteOutcome, NamespaceState, OwnedTempDisposition,
};
pub use event_log::{
    materialize_records, replay_conversation, AttachmentMaterialization, BindingEventPayloadV1,
    BindingMaterialization, BindingReplacementPayloadV1, ChunkedHistory, ConversationEventRecordV2,
    ConversationEventStream, ConversationEventType, ConversationReplay, EventLogError,
    EventLogErrorKind, EventLogRepairWarning, ExecutionTargetEventPayloadV1,
    ProjectAttachmentEventPayloadV1, ATTACHMENTS_FILE, BINDINGS_FILE,
    CONVERSATION_EVENT_SCHEMA_VERSION, EVENT_LOG_FILES, FRONTIER_HISTORY_CHUNK_ENTRIES,
    MESSAGES_FILE, TOOL_CALLS_FILE,
};
pub use lifecycle::{
    AgentLifecycleProviderError, AgentLifecycleProviderErrorKind, ConversationAgentLifecycle,
    ConversationDeleteBlocker, ConversationLifecycleAction, ConversationLifecycleError,
    ConversationLifecycleErrorCode, ConversationLifecycleOutcome, ConversationLifecycleService,
    TerminalResourceInspector,
};
pub use locator::{
    bounded_scan, BoundedScan, ConversationLocator, LocatedConversation, LocatorError,
    SessionWorkspaceLocator, MAX_CONVERSATIONS_PER_SCAN, MAX_DIRECTORY_ENTRIES_PER_LEVEL,
};
pub use migration::lock::{
    MigrationControlLock, MigrationControlLockGuard, MIGRATION_CONTROL_LOCK_FILE,
};
pub use migration::{
    advance_phase, recover_cutover, ActiveLayout, ApprovalReceiptV1, BootstrapObservationReceiptV1,
    CompatibilityError, ConversationLayoutDescriptorV1, ConversationMigrationControlService,
    ConversationMigrationService, ConversationReader, CutoverRecovery, HostMigrationLock,
    HostMigrationLockGuard, LegacyConversationProjection, LegacyConversationReader,
    MaintenanceReceiptState, MaintenanceRequestReceiptV1, MigrationAdmissionState,
    MigrationCallbacks, MigrationContext, MigrationControlContext, MigrationError,
    MigrationErrorCode, MigrationHostMode, MigrationJournalV1, MigrationMaintenanceAction,
    MigrationMaintenanceCompletionReceiptV1, MigrationMaintenanceRequestV1,
    MigrationMaintenanceScheduleReceiptV1, MigrationPhase, MigrationReport, MigrationStepOutput,
    ObservationEvidenceV1, ReaderPrecedence, StepReceiptV1,
};
pub use ordered_persistence::{
    OrderedConversationPersistence, OrderedPersistenceHealth, OrderedPersistenceMetrics,
    DEFAULT_DRAIN_TIMEOUT, GLOBAL_PENDING_BYTES, GLOBAL_PENDING_RECORDS,
    PER_SESSION_PENDING_RECORDS, QUEUE_CAPACITY, WRITER_SHARDS,
};
pub use persistence_adapter::{
    BindingMissCacheStats, ConversationPersistenceAdapter, ConversationPersistenceError,
    CONVERSATION_HISTORY_PAGING_REQUIRED, CONVERSATION_PERSISTENCE_COMMIT_INDETERMINATE,
    DEFAULT_DELIVERY_COMMIT_TIMEOUT, MAX_BINDING_MISS_CACHE_ENTRIES, MAX_COMPAT_HISTORY_RECORDS,
};
pub use repository::{
    CanonicalSequenceTicket, CatalogFlushCoordinator, CatalogFlushError, CatalogFlushFailureStage,
    CatalogFlushReceipt, ConversationAggregateMutationRecord, ConversationMetadataUpdate,
    ConversationRepository, RepositoryBindingIndexStats, RepositoryError, RepositoryOpenReport,
    RepositoryRecoveryItem, RepositoryRecoveryKind, CATALOG_FLUSH_DEBOUNCE,
    CATALOG_FLUSH_MAX_DELAY,
};
pub use session_workspace::{
    SessionWorkspaceError, SessionWorkspaceErrorCode, SessionWorkspaceLeafNode,
    SessionWorkspaceLoadOutcome, SessionWorkspacePaneDirection, SessionWorkspacePaneNode,
    SessionWorkspaceProjectionState, SessionWorkspaceResourceDescriptor, SessionWorkspaceService,
    SessionWorkspaceSplitNode, SessionWorkspaceV1, SessionWorkspaceWriteOutcome,
    TerminalResourceDescriptor, TerminalResourceRollbackFailure, SESSION_WORKSPACE_SCHEMA_VERSION,
    TERMINAL_RESOURCE_ROLLBACK_FAILED, TERMINAL_TERMINATE_FAILED,
};
pub use usage_plan::{
    validate_plan_update, validate_usage_update, PlanBodyV1, PlanEntryV1, PlanUpdateV1,
    UsageCostV1, UsagePlanSchemaError, UsageUpdateV1,
};
pub use workspace_projection::{
    LegacyWorkspaceProjector, WorkspaceProjectionOutcome, WorkspaceProjectionReceiptV1,
};
pub use write_authority::{ConversationMutation, ConversationWriteAuthority, ConversationWriter};

// Bootstrap-owned delivery contract consumed by ACP producers. Final integration injects the
// exact bootstrap coordinator Arc; these types remain transport-neutral and payload-free.
pub use crate::acp::events::{
    DeliveryError, DeliveryFailureClass, DeliveryReceipt, DeliveryTicket,
};
