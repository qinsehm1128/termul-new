use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::Utc;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::conversation::{DirectoryPermissions, DurableFileSystem};

use super::models::{
    ScheduledTaskAuditEventV1, ScheduledTaskCatalogV1, ScheduledTaskDraftInputV1,
    ScheduledTaskRunStatus, ScheduledTaskRunV1, ScheduledTaskStatus, ScheduledTaskV1,
    TaskMutationContextV1, SCHEDULED_TASK_AUDIT_SCHEMA_VERSION, SCHEDULED_TASK_RUN_SCHEMA_VERSION,
    SCHEDULED_TASK_SCHEMA_VERSION, SKILL_TEMPLATE_VERSION,
};
use super::schedule::{next_after, normalize_schedule, ScheduleError};

const CATALOG_FILE: &str = "tasks.json";
const RUNS_FILE: &str = "runs.jsonl";
const AUDIT_FILE: &str = "audit.jsonl";
/// New tasks are host-level resources. Keep them in one durable partition while
/// continuing to read legacy project-named partitions written by v1.
const GLOBAL_PARTITION: &str = "_global";
const MAX_NAME_BYTES: usize = 160;
const MAX_DESCRIPTION_BYTES: usize = 8 * 1024;
const MAX_PROMPT_BYTES: usize = 128 * 1024;
const MAX_PERMISSIONS: usize = 64;

#[derive(Debug)]
pub enum ScheduledTaskStoreError {
    Io(io::Error),
    Durable(crate::conversation::DurableFsError),
    Json(serde_json::Error),
    InvalidInput(String),
    InvalidSchedule(ScheduleError),
    NotFound(String),
    RevisionConflict { expected: u64, actual: u64 },
    DraftHashConflict,
    BadSchemaVersion { expected: u32, actual: u32 },
    Poisoned,
}

impl std::fmt::Display for ScheduledTaskStoreError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "scheduled task store io error: {error}"),
            Self::Durable(error) => {
                write!(formatter, "scheduled task durable write failed: {error}")
            }
            Self::Json(error) => write!(formatter, "scheduled task JSON is invalid: {error}"),
            Self::InvalidInput(detail) => {
                write!(formatter, "scheduled task input is invalid: {detail}")
            }
            Self::InvalidSchedule(error) => write!(formatter, "{error}"),
            Self::NotFound(id) => write!(formatter, "scheduled task was not found: {id}"),
            Self::RevisionConflict { expected, actual } => write!(
                formatter,
                "scheduled task revision conflict: expected {expected}, actual {actual}"
            ),
            Self::DraftHashConflict => {
                write!(formatter, "scheduled task draft changed after review")
            }
            Self::BadSchemaVersion { expected, actual } => write!(
                formatter,
                "scheduled task schema mismatch: expected {expected}, got {actual}"
            ),
            Self::Poisoned => write!(formatter, "scheduled task store lock is poisoned"),
        }
    }
}

impl std::error::Error for ScheduledTaskStoreError {}

impl From<io::Error> for ScheduledTaskStoreError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<serde_json::Error> for ScheduledTaskStoreError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

impl From<crate::conversation::DurableFsError> for ScheduledTaskStoreError {
    fn from(value: crate::conversation::DurableFsError) -> Self {
        Self::Durable(value)
    }
}

impl From<ScheduleError> for ScheduledTaskStoreError {
    fn from(value: ScheduleError) -> Self {
        Self::InvalidSchedule(value)
    }
}

pub type Result<T> = std::result::Result<T, ScheduledTaskStoreError>;

#[derive(Debug)]
pub struct ScheduledTaskStore {
    root: PathBuf,
    legacy_root: Option<PathBuf>,
    durable_fs: DurableFileSystem,
    mutation_lock: Mutex<()>,
}

#[derive(Debug)]
struct CatalogLocation {
    root: PathBuf,
    partition: String,
}

impl ScheduledTaskStore {
    pub fn open(root: PathBuf) -> Result<Self> {
        Self::open_with_legacy_root(root, None)
    }

    pub fn open_with_legacy_root(root: PathBuf, legacy_root: Option<PathBuf>) -> Result<Self> {
        let durable_fs = DurableFileSystem::new();
        durable_fs.create_dir_durable(&root, DirectoryPermissions::PrivateOwnerOnly)?;
        let legacy_root = legacy_root.filter(|legacy| legacy != &root && legacy.is_dir());
        if let Some(legacy) = legacy_root.as_ref() {
            log::info!(
                "[scheduled-task] boundary=legacy_store_enabled global_root={} legacy_root={}",
                root.display(),
                legacy.display()
            );
        }
        Ok(Self {
            root,
            legacy_root,
            durable_fs,
            mutation_lock: Mutex::new(()),
        })
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn list_tasks(&self, project_id: Option<&str>) -> Result<Vec<ScheduledTaskV1>> {
        let _guard = self
            .mutation_lock
            .lock()
            .map_err(|_| ScheduledTaskStoreError::Poisoned)?;
        if let Some(project_id) = project_id {
            validate_component(project_id, "projectId")?;
        }
        let mut tasks_by_id = BTreeMap::new();
        for root in self.catalog_roots() {
            for entry in fs::read_dir(root)? {
                let entry = entry?;
                if entry.file_type()?.is_dir() {
                    if let Some(partition) = entry.file_name().to_str() {
                        for task in self.load_catalog_from(root, partition)?.tasks {
                            // The global catalog is scanned first and wins if an
                            // interrupted migration left a duplicate legacy copy.
                            tasks_by_id.entry(task.task_id.clone()).or_insert(task);
                        }
                    }
                }
            }
        }
        let mut tasks = tasks_by_id.into_values().collect::<Vec<_>>();
        if let Some(project_id) = project_id {
            tasks.retain(|task| task.project_id.as_deref() == Some(project_id));
        }
        tasks.sort_by(|left, right| {
            left.project_id
                .cmp(&right.project_id)
                .then_with(|| left.name.cmp(&right.name))
        });
        Ok(tasks)
    }

    pub fn get_task(&self, task_id: &str) -> Result<ScheduledTaskV1> {
        validate_uuid(task_id, "taskId")?;
        self.list_tasks(None)?
            .into_iter()
            .find(|task| task.task_id == task_id)
            .ok_or_else(|| ScheduledTaskStoreError::NotFound(task_id.to_string()))
    }

    pub fn create_draft(
        &self,
        input: ScheduledTaskDraftInputV1,
        context: TaskMutationContextV1,
    ) -> Result<ScheduledTaskV1> {
        let _guard = self
            .mutation_lock
            .lock()
            .map_err(|_| ScheduledTaskStoreError::Poisoned)?;
        let input = validate_input(input)?;
        let now = Utc::now();
        let mut catalog = self.load_catalog(GLOBAL_PARTITION)?;
        let mut task = ScheduledTaskV1 {
            schema_version: SCHEDULED_TASK_SCHEMA_VERSION,
            task_id: Uuid::new_v4().to_string(),
            project_id: input.project_id.clone(),
            name: input.name,
            description: input.description,
            status: ScheduledTaskStatus::Draft,
            schedule: input.schedule,
            execution_policy: input.execution_policy,
            prompt: input.prompt,
            agent_config_id: input.agent_config_id,
            execution_target: input.execution_target,
            execution_cwd: input.execution_cwd,
            workspace_cwd: input.workspace_cwd,
            source_conversation_id: input.source_conversation_id,
            permissions: input.permissions,
            skill_template_version: SKILL_TEMPLATE_VERSION,
            revision: 1,
            draft_hash: String::new(),
            created_at: now.to_rfc3339(),
            updated_at: now.to_rfc3339(),
            next_run_at: None,
        };
        task.next_run_at = next_after(&task.schedule, now)?.map(|value| value.to_rfc3339());
        task.draft_hash = task_hash(&task)?;
        catalog.revision = catalog.revision.saturating_add(1);
        catalog.tasks.push(task.clone());
        self.write_catalog(GLOBAL_PARTITION, &catalog)?;
        self.append_audit_locked(
            GLOBAL_PARTITION,
            audit_for("draftCreated", None, Some(&task), context)?,
        )?;
        Ok(task)
    }

    pub fn update_draft(
        &self,
        task_id: &str,
        expected_revision: u64,
        input: ScheduledTaskDraftInputV1,
        context: TaskMutationContextV1,
    ) -> Result<ScheduledTaskV1> {
        validate_uuid(task_id, "taskId")?;
        let _guard = self
            .mutation_lock
            .lock()
            .map_err(|_| ScheduledTaskStoreError::Poisoned)?;
        let input = validate_input(input)?;
        let (location, mut catalog, index) = self.locate_task_locked(task_id)?;
        let before = catalog.tasks[index].clone();
        ensure_revision(&before, expected_revision)?;
        if before.status != ScheduledTaskStatus::Draft {
            return Err(ScheduledTaskStoreError::InvalidInput(
                "only draft tasks can be edited through updateDraft".to_string(),
            ));
        }
        let now = Utc::now();
        let task = &mut catalog.tasks[index];
        task.name = input.name;
        task.description = input.description;
        task.schedule = input.schedule;
        task.execution_policy = input.execution_policy;
        task.prompt = input.prompt;
        task.agent_config_id = input.agent_config_id;
        task.project_id = input.project_id;
        task.execution_target = input.execution_target;
        task.execution_cwd = input.execution_cwd;
        task.workspace_cwd = input.workspace_cwd;
        task.source_conversation_id = input.source_conversation_id;
        task.permissions = input.permissions;
        task.revision = task.revision.saturating_add(1);
        task.updated_at = now.to_rfc3339();
        task.next_run_at = next_after(&task.schedule, now)?.map(|value| value.to_rfc3339());
        task.draft_hash = task_hash(task)?;
        let updated = task.clone();
        catalog.revision = catalog.revision.saturating_add(1);
        self.write_catalog_at(&location.root, &location.partition, &catalog)?;
        self.append_audit_locked_at(
            &location.root,
            &location.partition,
            audit_for("draftUpdated", Some(&before), Some(&updated), context)?,
        )?;
        Ok(updated)
    }

    pub fn activate(
        &self,
        task_id: &str,
        expected_revision: u64,
        expected_draft_hash: &str,
        context: TaskMutationContextV1,
    ) -> Result<ScheduledTaskV1> {
        self.set_status(
            task_id,
            expected_revision,
            Some(expected_draft_hash),
            ScheduledTaskStatus::Active,
            "activated",
            context,
        )
    }

    pub fn pause(
        &self,
        task_id: &str,
        expected_revision: u64,
        context: TaskMutationContextV1,
    ) -> Result<ScheduledTaskV1> {
        self.set_status(
            task_id,
            expected_revision,
            None,
            ScheduledTaskStatus::Paused,
            "paused",
            context,
        )
    }

    pub fn resume(
        &self,
        task_id: &str,
        expected_revision: u64,
        context: TaskMutationContextV1,
    ) -> Result<ScheduledTaskV1> {
        self.set_status(
            task_id,
            expected_revision,
            None,
            ScheduledTaskStatus::Active,
            "resumed",
            context,
        )
    }

    pub fn delete(
        &self,
        task_id: &str,
        expected_revision: u64,
        context: TaskMutationContextV1,
    ) -> Result<()> {
        validate_uuid(task_id, "taskId")?;
        let _guard = self
            .mutation_lock
            .lock()
            .map_err(|_| ScheduledTaskStoreError::Poisoned)?;
        let (location, mut catalog, index) = self.locate_task_locked(task_id)?;
        let before = catalog.tasks[index].clone();
        ensure_revision(&before, expected_revision)?;
        catalog.tasks.remove(index);
        catalog.revision = catalog.revision.saturating_add(1);
        self.write_catalog_at(&location.root, &location.partition, &catalog)?;
        self.append_audit_locked_at(
            &location.root,
            &location.partition,
            audit_for("deleted", Some(&before), None, context)?,
        )?;
        Ok(())
    }

    /// Persist scheduler projection state without changing the user-facing
    /// task revision. This prevents a normal occurrence from invalidating a
    /// concurrent pause/edit CAS while still making `nextRunAt` crash durable.
    pub fn set_next_run_at(
        &self,
        task_id: &str,
        next_run_at: Option<String>,
    ) -> Result<ScheduledTaskV1> {
        validate_uuid(task_id, "taskId")?;
        let _guard = self
            .mutation_lock
            .lock()
            .map_err(|_| ScheduledTaskStoreError::Poisoned)?;
        let (location, mut catalog, index) = self.locate_task_locked(task_id)?;
        if catalog.tasks[index].next_run_at == next_run_at {
            return Ok(catalog.tasks[index].clone());
        }
        catalog.tasks[index].next_run_at = next_run_at;
        let updated = catalog.tasks[index].clone();
        catalog.revision = catalog.revision.saturating_add(1);
        self.write_catalog_at(&location.root, &location.partition, &catalog)?;
        Ok(updated)
    }

    pub fn append_run(&self, run: &ScheduledTaskRunV1) -> Result<()> {
        validate_run(run)?;
        let _guard = self
            .mutation_lock
            .lock()
            .map_err(|_| ScheduledTaskStoreError::Poisoned)?;
        let (location, _, _) = self.locate_task_locked(&run.task_id)?;
        let partition_dir = self.ensure_partition_dir_at(&location.root, &location.partition)?;
        let bytes = serde_json::to_vec(run)?;
        self.durable_fs
            .append_jsonl(&partition_dir.join(RUNS_FILE), &bytes)?;
        self.durable_fs
            .sync_file_and_namespace(&partition_dir.join(RUNS_FILE))?;
        Ok(())
    }

    pub fn list_runs(&self, task_id: &str) -> Result<Vec<ScheduledTaskRunV1>> {
        let _guard = self
            .mutation_lock
            .lock()
            .map_err(|_| ScheduledTaskStoreError::Poisoned)?;
        let (location, _, _) = self.locate_task_locked(task_id)?;
        let records = read_jsonl::<ScheduledTaskRunV1>(
            &self
                .partition_dir_at(&location.root, &location.partition)
                .join(RUNS_FILE),
        )?;
        let mut latest = BTreeMap::new();
        for run in records.into_iter().filter(|run| run.task_id == task_id) {
            validate_run(&run)?;
            latest.insert(run.run_id.clone(), run);
        }
        let mut runs = latest.into_values().collect::<Vec<_>>();
        runs.sort_by(|left, right| right.queued_at.cmp(&left.queued_at));
        Ok(runs)
    }

    pub fn list_audit(&self, task_id: &str) -> Result<Vec<ScheduledTaskAuditEventV1>> {
        let _guard = self
            .mutation_lock
            .lock()
            .map_err(|_| ScheduledTaskStoreError::Poisoned)?;
        let (location, _, _) = self.locate_task_locked(task_id)?;
        let mut events = read_jsonl::<ScheduledTaskAuditEventV1>(
            &self
                .partition_dir_at(&location.root, &location.partition)
                .join(AUDIT_FILE),
        )?
        .into_iter()
        .filter(|event| event.task_id == task_id)
        .collect::<Vec<_>>();
        events.sort_by(|left, right| left.created_at.cmp(&right.created_at));
        Ok(events)
    }

    fn set_status(
        &self,
        task_id: &str,
        expected_revision: u64,
        expected_draft_hash: Option<&str>,
        status: ScheduledTaskStatus,
        action: &str,
        context: TaskMutationContextV1,
    ) -> Result<ScheduledTaskV1> {
        validate_uuid(task_id, "taskId")?;
        let _guard = self
            .mutation_lock
            .lock()
            .map_err(|_| ScheduledTaskStoreError::Poisoned)?;
        let (location, mut catalog, index) = self.locate_task_locked(task_id)?;
        let before = catalog.tasks[index].clone();
        ensure_revision(&before, expected_revision)?;
        if let Some(expected_hash) = expected_draft_hash {
            if before.status != ScheduledTaskStatus::Draft || before.draft_hash != expected_hash {
                return Err(ScheduledTaskStoreError::DraftHashConflict);
            }
        }
        let task = &mut catalog.tasks[index];
        task.status = status;
        task.revision = task.revision.saturating_add(1);
        let now = Utc::now();
        task.updated_at = now.to_rfc3339();
        if status == ScheduledTaskStatus::Active {
            task.next_run_at = next_after(&task.schedule, now)?.map(|value| value.to_rfc3339());
        }
        task.draft_hash = task_hash(task)?;
        let updated = task.clone();
        catalog.revision = catalog.revision.saturating_add(1);
        self.write_catalog_at(&location.root, &location.partition, &catalog)?;
        self.append_audit_locked_at(
            &location.root,
            &location.partition,
            audit_for(action, Some(&before), Some(&updated), context)?,
        )?;
        Ok(updated)
    }

    fn locate_task_locked(
        &self,
        task_id: &str,
    ) -> Result<(CatalogLocation, ScheduledTaskCatalogV1, usize)> {
        for root in self.catalog_roots() {
            for entry in fs::read_dir(root)? {
                let entry = entry?;
                if !entry.file_type()?.is_dir() {
                    continue;
                }
                let Some(partition) = entry.file_name().to_str().map(str::to_string) else {
                    continue;
                };
                let catalog = self.load_catalog_from(root, &partition)?;
                if let Some(index) = catalog
                    .tasks
                    .iter()
                    .position(|task| task.task_id == task_id)
                {
                    return Ok((
                        CatalogLocation {
                            root: root.to_path_buf(),
                            partition,
                        },
                        catalog,
                        index,
                    ));
                }
            }
        }
        Err(ScheduledTaskStoreError::NotFound(task_id.to_string()))
    }

    fn catalog_roots(&self) -> Vec<&Path> {
        let mut roots = vec![self.root.as_path()];
        if let Some(legacy_root) = self.legacy_root.as_deref() {
            roots.push(legacy_root);
        }
        roots
    }

    fn load_catalog(&self, partition: &str) -> Result<ScheduledTaskCatalogV1> {
        self.load_catalog_from(&self.root, partition)
    }

    fn load_catalog_from(&self, root: &Path, partition: &str) -> Result<ScheduledTaskCatalogV1> {
        let path = self.partition_dir_at(root, partition).join(CATALOG_FILE);
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(ScheduledTaskCatalogV1::default())
            }
            Err(error) => return Err(error.into()),
        };
        let catalog = serde_json::from_slice::<ScheduledTaskCatalogV1>(&bytes)?;
        if catalog.schema_version != SCHEDULED_TASK_SCHEMA_VERSION {
            return Err(ScheduledTaskStoreError::BadSchemaVersion {
                expected: SCHEDULED_TASK_SCHEMA_VERSION,
                actual: catalog.schema_version,
            });
        }
        Ok(catalog)
    }

    fn write_catalog(&self, partition: &str, catalog: &ScheduledTaskCatalogV1) -> Result<()> {
        self.write_catalog_at(&self.root, partition, catalog)
    }

    fn write_catalog_at(
        &self,
        root: &Path,
        partition: &str,
        catalog: &ScheduledTaskCatalogV1,
    ) -> Result<()> {
        let partition_dir = self.ensure_partition_dir_at(root, partition)?;
        let bytes = serde_json::to_vec_pretty(catalog)?;
        self.durable_fs
            .replace_bytes(&partition_dir.join(CATALOG_FILE), &bytes)?;
        Ok(())
    }

    fn append_audit_locked(&self, partition: &str, event: ScheduledTaskAuditEventV1) -> Result<()> {
        self.append_audit_locked_at(&self.root, partition, event)
    }

    fn append_audit_locked_at(
        &self,
        root: &Path,
        partition: &str,
        event: ScheduledTaskAuditEventV1,
    ) -> Result<()> {
        let partition_dir = self.ensure_partition_dir_at(root, partition)?;
        let bytes = serde_json::to_vec(&event)?;
        let path = partition_dir.join(AUDIT_FILE);
        self.durable_fs.append_jsonl(&path, &bytes)?;
        self.durable_fs.sync_file_and_namespace(&path)?;
        Ok(())
    }

    fn ensure_partition_dir_at(&self, root: &Path, partition: &str) -> Result<PathBuf> {
        validate_component(partition, "scheduled task partition")?;
        let path = self.partition_dir_at(root, partition);
        self.durable_fs
            .create_dir_durable(&path, DirectoryPermissions::PrivateOwnerOnly)?;
        Ok(path)
    }

    fn partition_dir_at(&self, root: &Path, partition: &str) -> PathBuf {
        root.join(partition)
    }
}

fn validate_input(mut input: ScheduledTaskDraftInputV1) -> Result<ScheduledTaskDraftInputV1> {
    input.project_id = input
        .project_id
        .take()
        .map(|project_id| project_id.trim().to_string())
        .filter(|project_id| !project_id.is_empty());
    if let Some(project_id) = input.project_id.as_deref() {
        validate_component(project_id, "projectId")?;
    }
    input.name = input.name.trim().to_string();
    input.description = input.description.trim().to_string();
    input.prompt = input.prompt.trim().to_string();
    input.agent_config_id = input.agent_config_id.trim().to_string();
    input.execution_cwd = input.execution_cwd.trim().to_string();
    input.workspace_cwd = input.workspace_cwd.trim().to_string();
    if input.name.is_empty() || input.name.len() > MAX_NAME_BYTES {
        return Err(ScheduledTaskStoreError::InvalidInput(format!(
            "name must contain 1..={MAX_NAME_BYTES} bytes"
        )));
    }
    if input.description.len() > MAX_DESCRIPTION_BYTES {
        return Err(ScheduledTaskStoreError::InvalidInput(format!(
            "description exceeds {MAX_DESCRIPTION_BYTES} bytes"
        )));
    }
    if input.prompt.is_empty() || input.prompt.len() > MAX_PROMPT_BYTES {
        return Err(ScheduledTaskStoreError::InvalidInput(format!(
            "prompt must contain 1..={MAX_PROMPT_BYTES} bytes"
        )));
    }
    if input.agent_config_id.is_empty() {
        return Err(ScheduledTaskStoreError::InvalidInput(
            "agentConfigId is required".to_string(),
        ));
    }
    for (label, value) in [
        ("executionCwd", &input.execution_cwd),
        ("workspaceCwd", &input.workspace_cwd),
    ] {
        if !Path::new(value).is_absolute() {
            return Err(ScheduledTaskStoreError::InvalidInput(format!(
                "{label} must be absolute"
            )));
        }
    }
    if input.permissions.len() > MAX_PERMISSIONS {
        return Err(ScheduledTaskStoreError::InvalidInput(format!(
            "permissions exceeds {MAX_PERMISSIONS} entries"
        )));
    }
    let mut seen = HashSet::new();
    input.permissions.retain(|permission| {
        let permission = permission.trim();
        !permission.is_empty() && seen.insert(permission.to_string())
    });
    input.schedule = normalize_schedule(&input.schedule)?;
    if input.execution_policy.catch_up_window_seconds > 31 * 24 * 60 * 60 {
        return Err(ScheduledTaskStoreError::InvalidInput(
            "catchUpWindowSeconds must not exceed 31 days".to_string(),
        ));
    }
    Ok(input)
}

fn ensure_revision(task: &ScheduledTaskV1, expected: u64) -> Result<()> {
    if task.revision != expected {
        return Err(ScheduledTaskStoreError::RevisionConflict {
            expected,
            actual: task.revision,
        });
    }
    Ok(())
}

fn validate_component(value: &str, label: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        || value == "."
        || value == ".."
    {
        return Err(ScheduledTaskStoreError::InvalidInput(format!(
            "{label} contains an unsafe path component"
        )));
    }
    Ok(())
}

fn validate_uuid(value: &str, label: &str) -> Result<()> {
    Uuid::parse_str(value)
        .map_err(|_| ScheduledTaskStoreError::InvalidInput(format!("{label} must be a UUID")))?;
    Ok(())
}

fn validate_run(run: &ScheduledTaskRunV1) -> Result<()> {
    if run.schema_version != SCHEDULED_TASK_RUN_SCHEMA_VERSION {
        return Err(ScheduledTaskStoreError::BadSchemaVersion {
            expected: SCHEDULED_TASK_RUN_SCHEMA_VERSION,
            actual: run.schema_version,
        });
    }
    validate_uuid(&run.run_id, "runId")?;
    validate_uuid(&run.task_id, "taskId")?;
    if let Some(project_id) = run.project_id.as_deref() {
        validate_component(project_id, "projectId")?;
    }
    if run.occurrence_key.is_empty() || run.occurrence_key.len() > 256 {
        return Err(ScheduledTaskStoreError::InvalidInput(
            "occurrenceKey is invalid".to_string(),
        ));
    }
    Ok(())
}

fn task_hash(task: &ScheduledTaskV1) -> Result<String> {
    let mut canonical = task.clone();
    canonical.draft_hash.clear();
    let bytes = serde_json::to_vec(&canonical)?;
    Ok(hex_digest(&bytes))
}

pub fn snapshot_hash(task: &ScheduledTaskV1) -> Result<String> {
    task_hash(task)
}

fn audit_for(
    action: &str,
    before: Option<&ScheduledTaskV1>,
    after: Option<&ScheduledTaskV1>,
    context: TaskMutationContextV1,
) -> Result<ScheduledTaskAuditEventV1> {
    let task = after.or(before).ok_or_else(|| {
        ScheduledTaskStoreError::InvalidInput("audit event has no task".to_string())
    })?;
    Ok(ScheduledTaskAuditEventV1 {
        schema_version: SCHEDULED_TASK_AUDIT_SCHEMA_VERSION,
        event_id: Uuid::new_v4().to_string(),
        task_id: task.task_id.clone(),
        project_id: task.project_id.clone(),
        action: action.to_string(),
        actor: context.actor,
        source_conversation_id: context
            .source_conversation_id
            .or_else(|| task.source_conversation_id.clone()),
        source_tool_call_id: context.source_tool_call_id,
        before_hash: before.map(task_hash).transpose()?,
        after_hash: after.map(task_hash).transpose()?,
        created_at: Utc::now().to_rfc3339(),
    })
}

fn hex_digest(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn read_jsonl<T>(path: &Path) -> Result<Vec<T>>
where
    T: serde::de::DeserializeOwned,
{
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut values = Vec::new();
    for line in BufReader::new(file).lines() {
        let line = line?;
        if !line.trim().is_empty() {
            values.push(serde_json::from_str(&line)?);
        }
    }
    Ok(values)
}

pub fn new_queued_run(
    task: &ScheduledTaskV1,
    trigger: super::models::ScheduledTaskRunTrigger,
    scheduled_for: String,
    retry_of_run_id: Option<String>,
) -> Result<ScheduledTaskRunV1> {
    let occurrence_key = format!("{}:{scheduled_for}", task.task_id);
    Ok(ScheduledTaskRunV1 {
        schema_version: SCHEDULED_TASK_RUN_SCHEMA_VERSION,
        run_id: Uuid::new_v4().to_string(),
        task_id: task.task_id.clone(),
        project_id: task.project_id.clone(),
        trigger,
        status: ScheduledTaskRunStatus::Queued,
        occurrence_key,
        scheduled_for,
        queued_at: Utc::now().to_rfc3339(),
        started_at: None,
        finished_at: None,
        conversation_id: None,
        task_snapshot_hash: snapshot_hash(task)?,
        retry_of_run_id,
        summary: None,
        error_code: None,
        error_detail: None,
        usage: None,
    })
}

#[cfg(test)]
mod tests {
    use super::super::models::{CatchUpPolicy, ExecutionPolicyV1, OverlapPolicy, ScheduleSpecV1};
    use super::*;

    fn input(root: &Path) -> ScheduledTaskDraftInputV1 {
        ScheduledTaskDraftInputV1 {
            project_id: Some("project-a".to_string()),
            name: "Daily summary".to_string(),
            description: "Summarize work".to_string(),
            schedule: ScheduleSpecV1::Cron {
                expression: "0 9 * * *".to_string(),
                timezone: "Asia/Shanghai".to_string(),
            },
            execution_policy: ExecutionPolicyV1 {
                overlap: OverlapPolicy::BufferOne,
                catch_up: CatchUpPolicy::LatestOnce,
                catch_up_window_seconds: 86_400,
            },
            prompt: "Summarize this project".to_string(),
            agent_config_id: "codex-acp".to_string(),
            execution_target: crate::conversation::ExecutionTarget::ProjectRoot {
                project_id: "project-a".to_string(),
                project_root: root.to_string_lossy().into_owned(),
            },
            execution_cwd: root.to_string_lossy().into_owned(),
            workspace_cwd: root.to_string_lossy().into_owned(),
            source_conversation_id: None,
            permissions: Vec::new(),
        }
    }

    #[test]
    fn draft_requires_matching_hash_before_activation() {
        let root = fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!("termul-task-store-{}", Uuid::new_v4()));
        let store = ScheduledTaskStore::open(root.join("state")).unwrap();
        let draft = store
            .create_draft(input(&root), TaskMutationContextV1::default())
            .unwrap();
        assert!(matches!(
            store.activate(
                &draft.task_id,
                draft.revision,
                "wrong",
                TaskMutationContextV1::default()
            ),
            Err(ScheduledTaskStoreError::DraftHashConflict)
        ));
        let active = store
            .activate(
                &draft.task_id,
                draft.revision,
                &draft.draft_hash,
                TaskMutationContextV1::default(),
            )
            .unwrap();
        assert_eq!(active.status, ScheduledTaskStatus::Active);
        assert_eq!(store.list_audit(&active.task_id).unwrap().len(), 2);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn run_ledger_materializes_latest_record_per_run() {
        let root = fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!("termul-task-runs-{}", Uuid::new_v4()));
        let store = ScheduledTaskStore::open(root.join("state")).unwrap();
        let draft = store
            .create_draft(input(&root), TaskMutationContextV1::default())
            .unwrap();
        let mut run = new_queued_run(
            &draft,
            super::super::models::ScheduledTaskRunTrigger::Manual,
            Utc::now().to_rfc3339(),
            None,
        )
        .unwrap();
        store.append_run(&run).unwrap();
        run.status = ScheduledTaskRunStatus::Running;
        run.started_at = Some(Utc::now().to_rfc3339());
        store.append_run(&run).unwrap();
        let runs = store.list_runs(&draft.task_id).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].status, ScheduledTaskRunStatus::Running);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn projectless_draft_uses_global_partition_and_executes_normally() {
        let root = fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!("termul-task-global-{}", Uuid::new_v4()));
        let store = ScheduledTaskStore::open(root.join("state")).unwrap();
        let mut draft_input = input(&root);
        draft_input.project_id = None;
        draft_input.execution_target = crate::conversation::ExecutionTarget::Workspace;
        draft_input.source_conversation_id = Some(Uuid::new_v4().to_string());

        let draft = store
            .create_draft(draft_input, TaskMutationContextV1::default())
            .unwrap();
        assert_eq!(draft.project_id, None);
        assert!(store
            .root()
            .join(GLOBAL_PARTITION)
            .join(CATALOG_FILE)
            .is_file());
        assert_eq!(store.list_tasks(None).unwrap(), vec![draft.clone()]);
        assert!(store.list_tasks(Some("project-a")).unwrap().is_empty());

        let run = new_queued_run(
            &draft,
            super::super::models::ScheduledTaskRunTrigger::Manual,
            Utc::now().to_rfc3339(),
            None,
        )
        .unwrap();
        store.append_run(&run).unwrap();
        assert_eq!(store.list_runs(&draft.task_id).unwrap(), vec![run]);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn legacy_project_partitions_remain_readable_and_mutable() {
        let root = fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!("termul-task-legacy-{}", Uuid::new_v4()));
        let legacy_root = root.join("projects");
        let legacy_store = ScheduledTaskStore::open(legacy_root.clone()).unwrap();
        let original = legacy_store
            .create_draft(input(&root), TaskMutationContextV1::default())
            .unwrap();
        let catalog = legacy_store.load_catalog(GLOBAL_PARTITION).unwrap();
        legacy_store.write_catalog("project-a", &catalog).unwrap();
        fs::remove_dir_all(legacy_store.root().join(GLOBAL_PARTITION)).unwrap();
        drop(legacy_store);

        let global_root = root.join("catalog");
        let store = ScheduledTaskStore::open_with_legacy_root(
            global_root.clone(),
            Some(legacy_root.clone()),
        )
        .unwrap();

        assert_eq!(store.get_task(&original.task_id).unwrap(), original);
        let mut replacement = input(&root);
        replacement.name = "Updated legacy task".to_string();
        let updated = store
            .update_draft(
                &original.task_id,
                original.revision,
                replacement,
                TaskMutationContextV1::default(),
            )
            .unwrap();
        assert_eq!(updated.name, "Updated legacy task");
        assert!(legacy_root.join("project-a").join(CATALOG_FILE).is_file());
        assert!(!global_root.join("project-a").exists());
        let _ = fs::remove_dir_all(root);
    }
}
