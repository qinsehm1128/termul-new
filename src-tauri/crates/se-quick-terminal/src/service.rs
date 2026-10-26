use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{Datelike, SecondsFormat, Utc};
use se_foundation::durable_fs::{DirectoryPermissions, DurableFileSystem};
use se_pty::{PtyManager, SpawnOptions};
use serde::{Deserialize, Serialize};

use crate::record::{
    QuickTerminalOrigin, QuickTerminalRecord, QuickTerminalTarget, QUICK_TERMINAL_SCHEMA_VERSION,
};
use crate::store::QuickTerminalStore;
use crate::QuickTerminalId;

const MAX_TITLE_CHARS: usize = 200;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateQuickTerminal {
    pub target: QuickTerminalTarget,
    #[serde(default)]
    pub title: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OpenQuickTerminal {
    pub id: QuickTerminalId,
    pub cols: u16,
    pub rows: u16,
    #[serde(default)]
    pub terminal_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuickTerminalOpened {
    pub record: QuickTerminalRecord,
    pub terminal_id: String,
    /// Output claim for a PTY spawned by this call. `None` when an already
    /// running PTY was reused; the caller then resumes it like any other.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim: Option<String>,
    pub spawned: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuickTerminalError {
    NotFound,
    InvalidTarget(String),
    Storage(String),
    Spawn(String),
    Terminate(String),
}

impl QuickTerminalError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotFound => "QUICK_TERMINAL_NOT_FOUND",
            Self::InvalidTarget(_) => "QUICK_TERMINAL_INVALID_TARGET",
            Self::Storage(_) => "QUICK_TERMINAL_STORAGE_FAILED",
            Self::Spawn(_) => "QUICK_TERMINAL_SPAWN_FAILED",
            Self::Terminate(_) => "QUICK_TERMINAL_TERMINATE_FAILED",
        }
    }
}

impl fmt::Display for QuickTerminalError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => formatter.write_str("quick terminal not found"),
            Self::InvalidTarget(detail)
            | Self::Storage(detail)
            | Self::Spawn(detail)
            | Self::Terminate(detail) => formatter.write_str(detail),
        }
    }
}

impl std::error::Error for QuickTerminalError {}

type Result<T> = std::result::Result<T, QuickTerminalError>;

/// Quick terminal lifecycle over a store and the PTY runtime that owns shells.
pub struct QuickTerminalService {
    store: QuickTerminalStore,
    /// Parent of the per-terminal folders (`<base>/terminals/YYYY/MM/DD/<id>`).
    workspace_base: PathBuf,
    pty: Arc<PtyManager>,
    fs: DurableFileSystem,
    locks: parking_lot::Mutex<HashMap<QuickTerminalId, Arc<tokio::sync::Mutex<()>>>>,
    // Retain ownership if a persist failure is followed by a failed rollback.
    pending_cleanup: parking_lot::Mutex<HashMap<QuickTerminalId, Vec<String>>>,
    #[cfg(test)]
    fail_cleanup: parking_lot::Mutex<std::collections::HashSet<String>>,
}

impl QuickTerminalService {
    pub fn new(store: QuickTerminalStore, workspace_base: PathBuf, pty: Arc<PtyManager>) -> Self {
        Self {
            store,
            workspace_base,
            pty,
            fs: DurableFileSystem::new(),
            locks: parking_lot::Mutex::new(HashMap::new()),
            pending_cleanup: parking_lot::Mutex::new(HashMap::new()),
            #[cfg(test)]
            fail_cleanup: parking_lot::Mutex::new(std::collections::HashSet::new()),
        }
    }

    pub fn list(&self) -> Vec<QuickTerminalRecord> {
        self.store
            .list()
            .into_iter()
            .map(|record| self.live_record(self.stored_record(record.id).unwrap_or(record)))
            .collect()
    }

    pub fn get(&self, id: QuickTerminalId) -> Result<QuickTerminalRecord> {
        self.stored_record(id)
            .map(|record| self.live_record(record))
    }

    pub fn create(&self, request: CreateQuickTerminal) -> Result<QuickTerminalRecord> {
        let id = QuickTerminalId::new_v4();
        let now = Utc::now();
        let cwd = match &request.target {
            QuickTerminalTarget::Workspace => {
                let folder = self
                    .resolved_workspace_base()?
                    .join("terminals")
                    .join(format!(
                        "{:04}/{:02}/{:02}/{id}",
                        now.year(),
                        now.month(),
                        now.day()
                    ));
                self.fs
                    .create_dir_durable(&folder, DirectoryPermissions::Inherit)
                    .map_err(|error| QuickTerminalError::Storage(error.to_string()))?;
                path_to_string(&folder)?
            }
            QuickTerminalTarget::ProjectRoot {
                project_id,
                project_root,
            } => {
                require_project(project_id)?;
                existing_directory(project_root)?
            }
            QuickTerminalTarget::Worktree {
                project_id,
                worktree_path,
                worktree_branch,
            } => {
                require_project(project_id)?;
                if worktree_branch.trim().is_empty() {
                    return Err(QuickTerminalError::InvalidTarget(
                        "worktree branch is empty".to_string(),
                    ));
                }
                existing_directory(worktree_path)?
            }
        };
        let stamp = timestamp(now);
        let record = QuickTerminalRecord {
            schema_version: QUICK_TERMINAL_SCHEMA_VERSION,
            id,
            title: normalize_title(request.title),
            target: request.target,
            cwd,
            created_at_utc: stamp.clone(),
            updated_at_utc: stamp,
            terminal_id: None,
            terminal_ids: Vec::new(),
            origin: QuickTerminalOrigin::Created,
        };
        self.store.put(record.clone()).map_err(storage)?;
        log::info!(
            target: "se_manager::quick_terminal",
            "operation=create id={id} target={} stable_code=OK",
            target_kind(&record.target)
        );
        Ok(record)
    }

    /// Reuse a specific owned live shell, or the first remaining live shell.
    pub async fn open(&self, request: OpenQuickTerminal) -> Result<QuickTerminalOpened> {
        let lock = self.lock_for(request.id);
        let _guard = lock.lock().await;
        self.retry_pending(request.id).await?;
        let mut record = self.stored_record(request.id)?;
        if let Some(target) = &request.terminal_id {
            self.require_owned(&record, target)?;
        }
        self.reconcile(&mut record).await?;
        let selected = request.terminal_id.as_ref().or(record.terminal_ids.first());
        if let Some(terminal_id) = selected.cloned() {
            if !record.terminal_ids.contains(&terminal_id) {
                return Err(invalid_terminal());
            }
            return Ok(QuickTerminalOpened {
                record,
                terminal_id,
                claim: None,
                spawned: false,
            });
        }
        self.spawn_shell(record, request.cols, request.rows, "open")
            .await
    }

    /// Explicitly add one shell without changing the primary/legacy alias.
    pub async fn create_terminal(
        &self,
        id: QuickTerminalId,
        cols: u16,
        rows: u16,
    ) -> Result<QuickTerminalOpened> {
        let lock = self.lock_for(id);
        let _guard = lock.lock().await;
        self.retry_pending(id).await?;
        let mut record = self.stored_record(id)?;
        self.reconcile(&mut record).await?;
        self.spawn_shell(record, cols, rows, "create_terminal")
            .await
    }

    async fn spawn_shell(
        &self,
        mut record: QuickTerminalRecord,
        cols: u16,
        rows: u16,
        operation: &str,
    ) -> Result<QuickTerminalOpened> {
        if cols == 0 || rows == 0 {
            return Err(QuickTerminalError::InvalidTarget(
                "terminal dimensions must be greater than zero".to_string(),
            ));
        }
        if !Path::new(&record.cwd).is_dir() {
            if record.target != QuickTerminalTarget::Workspace {
                return Err(QuickTerminalError::InvalidTarget(
                    "the quick terminal's directory no longer exists".to_string(),
                ));
            }
            self.fs
                .create_dir_durable(Path::new(&record.cwd), DirectoryPermissions::Inherit)
                .map_err(|error| QuickTerminalError::Storage(error.to_string()))?;
        }
        let spawned = self
            .pty
            .spawn(
                SpawnOptions {
                    cwd: Some(record.cwd.clone()),
                    conversation_id: Some(record.id),
                    // Never adopt a quick shell into a project's terminal catalog.
                    project_id: None,
                    cols: Some(cols),
                    rows: Some(rows),
                    ..Default::default()
                },
                None,
            )
            .await
            .map_err(QuickTerminalError::Spawn)?;
        let terminal_id = spawned.info.id.clone();
        record.terminal_ids.push(terminal_id.clone());
        record.sync_primary();
        record.updated_at_utc = timestamp(Utc::now());
        if let Err(error) = self.store.put(record.clone()) {
            log::error!(target: "se_manager::quick_terminal",
                "operation={operation} id={} terminal_id={terminal_id} stable_code=PERSIST_FAILED_ROLLBACK", record.id);
            if let Err(failure) = self
                .end_shell(record.id, &terminal_id, "spawn_rollback")
                .await
            {
                log::error!(target: "se_manager::quick_terminal",
                    "operation=spawn_rollback id={} terminal_id={terminal_id} stable_code=QUICK_TERMINAL_TERMINATE_FAILED", record.id);
                self.pending_cleanup
                    .lock()
                    .entry(record.id)
                    .or_default()
                    .push(terminal_id.clone());
                return Err(failure);
            }
            return Err(storage(error));
        }
        log::info!(target: "se_manager::quick_terminal",
            "operation={operation} id={} terminal_id={terminal_id} spawned=true stable_code=DURABLE_OK", record.id);
        Ok(QuickTerminalOpened {
            record,
            terminal_id,
            claim: Some(spawned.claim),
            spawned: true,
        })
    }

    pub async fn rename(
        &self,
        id: QuickTerminalId,
        title: Option<String>,
    ) -> Result<QuickTerminalRecord> {
        let lock = self.lock_for(id);
        let _guard = lock.lock().await;
        self.retry_pending(id).await?;
        let mut record = self.stored_record(id)?;
        self.reconcile(&mut record).await?;
        record.title = normalize_title(title);
        record.updated_at_utc = timestamp(Utc::now());
        self.store.put(record.clone()).map_err(storage)?;
        log::info!(target: "se_manager::quick_terminal", "operation=rename id={id} stable_code=DURABLE_OK");
        Ok(record)
    }

    /// Close only the selected shell, leaving all siblings running.
    pub async fn close_terminal(
        &self,
        id: QuickTerminalId,
        terminal_id: &str,
    ) -> Result<QuickTerminalRecord> {
        let lock = self.lock_for(id);
        let _guard = lock.lock().await;
        self.retry_pending(id).await?;
        let mut record = self.stored_record(id)?;
        self.require_owned(&record, terminal_id)?;
        self.end_shell(id, terminal_id, "close_terminal").await?;
        record.terminal_ids.retain(|owned| owned != terminal_id);
        self.reconcile(&mut record).await?;
        record.sync_primary();
        record.updated_at_utc = timestamp(Utc::now());
        self.store.put(record.clone()).map_err(storage)?;
        log::info!(target: "se_manager::quick_terminal",
            "operation=close_terminal id={id} terminal_id={terminal_id} stable_code=DURABLE_OK");
        Ok(record)
    }

    /// End all owned shells but retain the entry and its folder.
    pub async fn close(&self, id: QuickTerminalId) -> Result<QuickTerminalRecord> {
        let lock = self.lock_for(id);
        let _guard = lock.lock().await;
        let mut record = self.stored_record(id)?;
        if record.terminal_ids.is_empty() {
            return Ok(record);
        }
        self.end_all(&record, "close").await?;
        record.terminal_ids.clear();
        record.sync_primary();
        record.updated_at_utc = timestamp(Utc::now());
        self.store.put(record.clone()).map_err(storage)?;
        log::info!(target: "se_manager::quick_terminal", "operation=close id={id} stable_code=DURABLE_OK");
        Ok(record)
    }

    /// Remove the entry only after every owned shell is safely cleaned up.
    pub async fn delete(&self, id: QuickTerminalId) -> Result<()> {
        let lock = self.lock_for(id);
        let _guard = lock.lock().await;
        let record = self.stored_record(id)?;
        self.end_all(&record, "delete").await?;
        self.store.remove(id).map_err(storage)?;
        // Keep the gate: queued operations must never switch to a new lock.
        log::info!(target: "se_manager::quick_terminal", "operation=delete id={id} stable_code=DURABLE_OK");
        Ok(())
    }

    async fn end_all(&self, record: &QuickTerminalRecord, operation: &str) -> Result<()> {
        // Validate every target before touching any shell.
        for terminal_id in &record.terminal_ids {
            self.require_owned(record, terminal_id)?;
        }
        let mut first_failure = None;
        for terminal_id in &record.terminal_ids {
            match self.end_shell(record.id, terminal_id, operation).await {
                Ok(()) => self.forget_pending(record.id, terminal_id),
                Err(error) => {
                    first_failure.get_or_insert(error);
                }
            }
        }
        first_failure.map_or(Ok(()), Err)
    }

    fn forget_pending(&self, id: QuickTerminalId, terminal_id: &str) {
        let mut pending = self.pending_cleanup.lock();
        if let Some(ids) = pending.get_mut(&id) {
            ids.retain(|owned| owned != terminal_id);
            if ids.is_empty() {
                pending.remove(&id);
            }
        }
    }

    async fn retry_pending(&self, id: QuickTerminalId) -> Result<()> {
        let ids = self
            .pending_cleanup
            .lock()
            .get(&id)
            .cloned()
            .unwrap_or_default();
        let mut first_failure = None;
        for terminal_id in ids {
            match self.end_shell(id, &terminal_id, "pending_rollback").await {
                Ok(()) => self.forget_pending(id, &terminal_id),
                Err(error) => {
                    first_failure.get_or_insert(error);
                }
            }
        }
        first_failure.map_or(Ok(()), Err)
    }

    fn stored_record(&self, id: QuickTerminalId) -> Result<QuickTerminalRecord> {
        let mut record = self.store.get(id).ok_or(QuickTerminalError::NotFound)?;
        if let Some(pending) = self.pending_cleanup.lock().get(&id) {
            for terminal_id in pending {
                if !record.terminal_ids.contains(terminal_id) {
                    record.terminal_ids.push(terminal_id.clone());
                }
            }
        }
        record.sync_primary();
        Ok(record)
    }

    fn require_owned(&self, record: &QuickTerminalRecord, terminal_id: &str) -> Result<()> {
        if !record.terminal_ids.iter().any(|id| id == terminal_id) {
            return Err(invalid_terminal());
        }
        self.require_runtime_owner(record.id, terminal_id)
    }

    fn require_runtime_owner(&self, id: QuickTerminalId, terminal_id: &str) -> Result<()> {
        if self
            .pty
            .get(terminal_id)
            .is_some_and(|instance| instance.conversation_id != id || instance.project_id.is_some())
        {
            log::warn!(target: "se_manager::quick_terminal",
                "operation=ownership id={id} terminal_id={terminal_id} stable_code=QUICK_TERMINAL_INVALID_TARGET");
            return Err(invalid_terminal());
        }
        Ok(())
    }

    fn live_record(&self, mut record: QuickTerminalRecord) -> QuickTerminalRecord {
        record.terminal_ids.retain(|terminal_id| {
            self.require_runtime_owner(record.id, terminal_id).is_ok()
                && self.shell_is_running(terminal_id)
        });
        record.sync_primary();
        record
    }

    async fn reconcile(&self, record: &mut QuickTerminalRecord) -> Result<()> {
        let mut remaining = Vec::new();
        for terminal_id in &record.terminal_ids {
            self.require_owned(record, terminal_id)?;
            if self.shell_is_running(terminal_id) {
                remaining.push(terminal_id.clone());
            } else {
                self.end_shell(record.id, terminal_id, "stale_cleanup")
                    .await?;
            }
        }
        if record.terminal_ids != remaining {
            record.terminal_ids = remaining;
            record.sync_primary();
            record.updated_at_utc = timestamp(Utc::now());
            self.store.put(record.clone()).map_err(storage)?;
        }
        Ok(())
    }

    async fn end_shell(
        &self,
        id: QuickTerminalId,
        terminal_id: &str,
        operation: &str,
    ) -> Result<()> {
        self.require_runtime_owner(id, terminal_id)?;
        #[cfg(test)]
        if self.fail_cleanup.lock().contains(terminal_id) || self.fail_cleanup.lock().contains("*")
        {
            return Err(QuickTerminalError::Terminate(
                "injected isolated fixture cleanup failure".into(),
            ));
        }
        if self.pty.get(terminal_id).is_none() {
            return Ok(());
        }
        self.pty.terminate(terminal_id).await.map(|_| ()).map_err(|failure| {
            log::error!(
                target: "se_manager::quick_terminal",
                "operation={operation} id={id} terminal_id={terminal_id} stable_code=QUICK_TERMINAL_TERMINATE_FAILED"
            );
            QuickTerminalError::Terminate(format!("{failure:?}"))
        })
    }

    /// Adopt a record created elsewhere (migration). Idempotent: an existing
    /// record with the same id wins and `false` is returned.
    pub fn import(&self, record: QuickTerminalRecord) -> Result<bool> {
        if self.store.get(record.id).is_some() {
            return Ok(false);
        }
        self.store.put(record).map_err(storage)?;
        Ok(true)
    }

    /// The PTY is registered as active *and* its shell has not exited. An
    /// exited shell keeps its `Active` registration until cleanup runs.
    fn shell_is_running(&self, terminal_id: &str) -> bool {
        self.pty
            .get(terminal_id)
            .is_some_and(|instance| instance.is_active())
            && !self.pty.terminal_events().snapshot(terminal_id).exited
    }

    /// The configured base, created if missing and resolved to its real path.
    ///
    /// The base is trusted configuration but may sit behind a symlink (macOS
    /// `/tmp`, a synced Documents folder). Durable creation refuses symlinked
    /// components, so only the folders created below the base are held to that.
    fn resolved_workspace_base(&self) -> Result<PathBuf> {
        std::fs::create_dir_all(&self.workspace_base)
            .and_then(|()| self.workspace_base.canonicalize())
            .map_err(|error| {
                QuickTerminalError::Storage(format!(
                    "the quick terminal folder root is unavailable: {error}"
                ))
            })
    }

    fn lock_for(&self, id: QuickTerminalId) -> Arc<tokio::sync::Mutex<()>> {
        Arc::clone(self.locks.lock().entry(id).or_default())
    }
}

fn invalid_terminal() -> QuickTerminalError {
    QuickTerminalError::InvalidTarget("terminal is not an owned live quick terminal".to_string())
}

fn storage(error: std::io::Error) -> QuickTerminalError {
    log::error!(target: "se_manager::quick_terminal", "operation=store_write stable_code=QUICK_TERMINAL_STORAGE_FAILED kind={:?}", error.kind());
    QuickTerminalError::Storage(error.to_string())
}

fn timestamp(now: chrono::DateTime<Utc>) -> String {
    now.to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn normalize_title(title: Option<String>) -> Option<String> {
    let title = title?.trim().to_string();
    if title.is_empty() {
        return None;
    }
    Some(title.chars().take(MAX_TITLE_CHARS).collect())
}

fn require_project(project_id: &str) -> Result<()> {
    if project_id.trim().is_empty() {
        return Err(QuickTerminalError::InvalidTarget(
            "project id is empty".to_string(),
        ));
    }
    Ok(())
}

fn existing_directory(path: &str) -> Result<String> {
    let resolved = Path::new(path).canonicalize().map_err(|_| {
        QuickTerminalError::InvalidTarget("the selected directory does not exist".to_string())
    })?;
    if !resolved.is_dir() {
        return Err(QuickTerminalError::InvalidTarget(
            "the selected path is not a directory".to_string(),
        ));
    }
    path_to_string(&resolved)
}

fn path_to_string(path: &Path) -> Result<String> {
    path.to_str()
        .map(|value| se_foundation::path_validation::strip_verbatim_prefix(value).into_owned())
        .ok_or_else(|| QuickTerminalError::InvalidTarget("path is not valid UTF-8".to_string()))
}

fn target_kind(target: &QuickTerminalTarget) -> &'static str {
    match target {
        QuickTerminalTarget::Workspace => "workspace",
        QuickTerminalTarget::ProjectRoot { .. } => "project_root",
        QuickTerminalTarget::Worktree { .. } => "worktree",
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    struct Fixture {
        _state: tempfile::TempDir,
        base: tempfile::TempDir,
        pty: Arc<PtyManager>,
        service: QuickTerminalService,
    }

    fn fixture() -> Fixture {
        let state = tempfile::tempdir().unwrap();
        let base = tempfile::tempdir().unwrap();
        let pty = se_pty::test_pty_manager();
        // Durable directory creation refuses symlinked components (macOS `/var`).
        let store =
            QuickTerminalStore::open(state.path().canonicalize().unwrap().join("quick-terminals"))
                .unwrap();
        let service =
            QuickTerminalService::new(store, base.path().canonicalize().unwrap(), Arc::clone(&pty));
        Fixture {
            _state: state,
            base,
            pty,
            service,
        }
    }

    fn open(id: QuickTerminalId) -> OpenQuickTerminal {
        OpenQuickTerminal {
            id,
            cols: 80,
            rows: 24,
            terminal_id: None,
        }
    }

    #[tokio::test]
    async fn workspace_target_gets_its_own_folder_and_shell() {
        let fixture = fixture();
        let record = fixture
            .service
            .create(CreateQuickTerminal {
                target: QuickTerminalTarget::Workspace,
                title: Some("  scratch  ".to_string()),
            })
            .unwrap();

        let terminals = fixture
            .base
            .path()
            .canonicalize()
            .unwrap()
            .join("terminals");
        assert!(record.cwd.starts_with(terminals.to_str().unwrap()));
        assert!(record.cwd.ends_with(&record.id.to_string()));
        assert!(Path::new(&record.cwd).is_dir());
        assert_eq!(record.title.as_deref(), Some("scratch"));

        let opened = fixture.service.open(open(record.id)).await.unwrap();
        assert!(opened.spawned);
        assert!(opened.claim.is_some());
        let instance = fixture.pty.get(&opened.terminal_id).unwrap();
        assert_eq!(instance.cwd, record.cwd);
        assert_eq!(
            fixture.service.get(record.id).unwrap().terminal_id,
            Some(opened.terminal_id.clone())
        );
        let _ = fixture.pty.terminate(&opened.terminal_id).await;
    }

    #[test]
    fn a_workspace_base_reached_through_a_symlink_still_gets_folders() {
        // The configured base is trusted, but it may sit behind a symlink
        // (macOS `/tmp` → `/private/tmp`, a synced Documents folder). Durable
        // creation refuses symlinked components, so the base is resolved first.
        let state = tempfile::tempdir().unwrap();
        let real = tempfile::tempdir().unwrap();
        let link_parent = tempfile::tempdir().unwrap();
        let link = link_parent.path().join("workspace-link");
        std::os::unix::fs::symlink(real.path(), &link).unwrap();
        let store =
            QuickTerminalStore::open(state.path().canonicalize().unwrap().join("quick-terminals"))
                .unwrap();
        let service = QuickTerminalService::new(store, link.clone(), se_pty::test_pty_manager());

        let record = service
            .create(CreateQuickTerminal {
                target: QuickTerminalTarget::Workspace,
                title: None,
            })
            .unwrap();

        assert!(Path::new(&record.cwd).is_dir());
        assert!(record
            .cwd
            .starts_with(real.path().canonicalize().unwrap().to_str().unwrap()));
    }

    #[tokio::test]
    async fn reopening_reuses_the_live_shell_and_replaces_a_dead_one() {
        let fixture = fixture();
        let record = fixture
            .service
            .create(CreateQuickTerminal {
                target: QuickTerminalTarget::Workspace,
                title: None,
            })
            .unwrap();
        let first = fixture.service.open(open(record.id)).await.unwrap();

        let again = fixture.service.open(open(record.id)).await.unwrap();
        assert!(!again.spawned);
        assert_eq!(again.terminal_id, first.terminal_id);
        assert!(again.claim.is_none());

        fixture.pty.terminate(&first.terminal_id).await.unwrap();
        let replaced = fixture.service.open(open(record.id)).await.unwrap();
        assert!(replaced.spawned);
        assert_ne!(replaced.terminal_id, first.terminal_id);
        let _ = fixture.pty.terminate(&replaced.terminal_id).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_opens_start_one_shell() {
        let Fixture {
            _state,
            base: _base,
            pty,
            service,
        } = fixture();
        let service = Arc::new(service);
        let record = service
            .create(CreateQuickTerminal {
                target: QuickTerminalTarget::Workspace,
                title: None,
            })
            .unwrap();

        let barrier = Arc::new(tokio::sync::Barrier::new(8));
        let opens: Vec<_> = (0..8)
            .map(|_| {
                let service = Arc::clone(&service);
                let barrier = Arc::clone(&barrier);
                tokio::spawn(async move {
                    barrier.wait().await;
                    service.open(open(record.id)).await.unwrap()
                })
            })
            .collect();
        let mut opened = Vec::new();
        for task in opens {
            opened.push(task.await.unwrap());
        }

        assert_eq!(opened.iter().filter(|result| result.spawned).count(), 1);
        assert!(opened
            .iter()
            .all(|result| result.terminal_id == opened[0].terminal_id));
        let _ = pty.terminate(&opened[0].terminal_id).await;
    }

    #[tokio::test]
    async fn a_shell_that_exited_on_its_own_is_replaced() {
        let fixture = fixture();
        let record = fixture
            .service
            .create(CreateQuickTerminal {
                target: QuickTerminalTarget::Workspace,
                title: None,
            })
            .unwrap();
        let first = fixture.service.open(open(record.id)).await.unwrap();
        fixture
            .pty
            .write(&first.terminal_id, "exit\n")
            .await
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while !fixture
                .pty
                .terminal_events()
                .snapshot(&first.terminal_id)
                .exited
            {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("the shell exits");

        let replaced = fixture.service.open(open(record.id)).await.unwrap();
        assert!(replaced.spawned);
        assert_ne!(replaced.terminal_id, first.terminal_id);
        let _ = fixture.pty.terminate(&replaced.terminal_id).await;
    }

    #[tokio::test]
    async fn delete_ends_the_shell_and_keeps_the_folder() {
        let fixture = fixture();
        let record = fixture
            .service
            .create(CreateQuickTerminal {
                target: QuickTerminalTarget::Workspace,
                title: None,
            })
            .unwrap();
        let opened = fixture.service.open(open(record.id)).await.unwrap();

        fixture.service.delete(record.id).await.unwrap();

        assert!(fixture
            .pty
            .get(&opened.terminal_id)
            .is_none_or(|instance| !instance.is_active()));
        assert_eq!(
            fixture.service.get(record.id),
            Err(QuickTerminalError::NotFound)
        );
        assert!(Path::new(&record.cwd).is_dir());
    }

    #[tokio::test]
    async fn close_ends_the_shell_keeps_the_record_and_reopen_starts_a_new_one() {
        let fixture = fixture();
        let record = fixture
            .service
            .create(CreateQuickTerminal {
                target: QuickTerminalTarget::Workspace,
                title: Some("build".to_string()),
            })
            .unwrap();
        let opened = fixture.service.open(open(record.id)).await.unwrap();

        let closed = fixture.service.close(record.id).await.unwrap();

        assert!(fixture
            .pty
            .get(&opened.terminal_id)
            .is_none_or(|instance| !instance.is_active()));
        assert_eq!(closed.terminal_id, None);
        assert_eq!(fixture.service.get(record.id).unwrap(), closed);
        assert_eq!(closed.title.as_deref(), Some("build"));
        // Closing again has nothing to end.
        assert_eq!(fixture.service.close(record.id).await.unwrap(), closed);

        let reopened = fixture.service.open(open(record.id)).await.unwrap();
        assert!(reopened.spawned);
        assert_ne!(reopened.terminal_id, opened.terminal_id);
        fixture.service.delete(record.id).await.unwrap();
    }

    #[test]
    fn project_targets_must_point_at_an_existing_directory() {
        let fixture = fixture();
        let missing = fixture.service.create(CreateQuickTerminal {
            target: QuickTerminalTarget::ProjectRoot {
                project_id: "p1".to_string(),
                project_root: "/definitely/not/here".to_string(),
            },
            title: None,
        });
        assert_eq!(missing.unwrap_err().code(), "QUICK_TERMINAL_INVALID_TARGET");

        let project = tempfile::tempdir().unwrap();
        let record = fixture
            .service
            .create(CreateQuickTerminal {
                target: QuickTerminalTarget::ProjectRoot {
                    project_id: "p1".to_string(),
                    project_root: project.path().to_str().unwrap().to_string(),
                },
                title: None,
            })
            .unwrap();
        assert_eq!(
            Path::new(&record.cwd),
            project.path().canonicalize().unwrap()
        );
        assert_eq!(record.target.project_id(), Some("p1"));
    }

    #[tokio::test]
    async fn project_shells_are_not_announced_as_project_terminals() {
        let fixture = fixture();
        let project = tempfile::tempdir().unwrap();
        let record = fixture
            .service
            .create(CreateQuickTerminal {
                target: QuickTerminalTarget::ProjectRoot {
                    project_id: "p1".to_string(),
                    project_root: project.path().to_str().unwrap().to_string(),
                },
                title: None,
            })
            .unwrap();
        let opened = fixture.service.open(open(record.id)).await.unwrap();

        let instance = fixture.pty.get(&opened.terminal_id).unwrap();
        assert_eq!(instance.project_id, None);
        assert_eq!(instance.cwd, record.cwd);
        let _ = fixture.pty.terminate(&opened.terminal_id).await;
    }

    #[test]
    fn import_keeps_an_existing_record() {
        let fixture = fixture();
        let created = fixture
            .service
            .create(CreateQuickTerminal {
                target: QuickTerminalTarget::Workspace,
                title: Some("mine".to_string()),
            })
            .unwrap();
        let mut duplicate = created.clone();
        duplicate.title = Some("other".to_string());

        assert!(!fixture.service.import(duplicate).unwrap());
        assert_eq!(
            fixture.service.get(created.id).unwrap().title.as_deref(),
            Some("mine")
        );
    }
    fn entry(fixture: &Fixture) -> QuickTerminalRecord {
        fixture
            .service
            .create(CreateQuickTerminal {
                target: QuickTerminalTarget::Workspace,
                title: None,
            })
            .unwrap()
    }

    #[tokio::test]
    async fn multiple_shells_reuse_selection_and_close_only_the_owned_target() {
        let f = fixture();
        let record = entry(&f);
        let first = f.service.open(open(record.id)).await.unwrap();
        let second = f.service.create_terminal(record.id, 90, 30).await.unwrap();
        assert_ne!(first.terminal_id, second.terminal_id);
        assert!(second.spawned && second.claim.is_some());
        assert_eq!(
            second.record.terminal_ids,
            vec![first.terminal_id.clone(), second.terminal_id.clone()]
        );
        assert_eq!(second.record.terminal_id, Some(first.terminal_id.clone()));
        for terminal_id in &second.record.terminal_ids {
            let instance = f.pty.get(terminal_id).unwrap();
            assert_eq!(instance.cwd, record.cwd);
            assert_eq!(instance.conversation_id, record.id);
            assert_eq!(instance.project_id, None);
        }
        let primary = f.service.open(open(record.id)).await.unwrap();
        assert!(!primary.spawned);
        assert_eq!(primary.terminal_id, first.terminal_id);
        let mut selected = open(record.id);
        selected.terminal_id = Some(second.terminal_id.clone());
        let reused = f.service.open(selected).await.unwrap();
        assert!(!reused.spawned && reused.claim.is_none());
        assert_eq!(reused.terminal_id, second.terminal_id);
        let closed = f
            .service
            .close_terminal(record.id, &first.terminal_id)
            .await
            .unwrap();
        assert_eq!(closed.terminal_ids, vec![second.terminal_id.clone()]);
        assert_eq!(closed.terminal_id, Some(second.terminal_id.clone()));
        assert!(!f.service.shell_is_running(&first.terminal_id));
        assert!(f.service.shell_is_running(&second.terminal_id));
        f.service.delete(record.id).await.unwrap();
        assert!(!f.service.shell_is_running(&second.terminal_id));
    }

    #[tokio::test]
    async fn close_all_and_delete_end_every_shell() {
        let f = fixture();
        let record = entry(&f);
        let first = f.service.create_terminal(record.id, 80, 24).await.unwrap();
        let second = f.service.create_terminal(record.id, 80, 24).await.unwrap();
        let closed = f.service.close(record.id).await.unwrap();
        assert!(closed.terminal_ids.is_empty() && closed.terminal_id.is_none());
        assert!(!f.service.shell_is_running(&first.terminal_id));
        assert!(!f.service.shell_is_running(&second.terminal_id));
        let third = f.service.create_terminal(record.id, 80, 24).await.unwrap();
        let fourth = f.service.create_terminal(record.id, 80, 24).await.unwrap();
        f.service.delete(record.id).await.unwrap();
        assert!(!f.service.shell_is_running(&third.terminal_id));
        assert!(!f.service.shell_is_running(&fourth.terminal_id));
        assert_eq!(f.service.get(record.id), Err(QuickTerminalError::NotFound));
    }

    #[tokio::test]
    async fn unknown_and_forged_foreign_ownership_never_adopt_or_kill() {
        let f = fixture();
        let own = entry(&f);
        let foreign = entry(&f);
        let theirs = f.service.open(open(foreign.id)).await.unwrap();
        let mut request = open(own.id);
        request.terminal_id = Some(theirs.terminal_id.clone());
        assert_eq!(
            f.service.open(request).await.unwrap_err().code(),
            "QUICK_TERMINAL_INVALID_TARGET"
        );
        assert_eq!(
            f.service
                .close_terminal(own.id, &theirs.terminal_id)
                .await
                .unwrap_err()
                .code(),
            "QUICK_TERMINAL_INVALID_TARGET"
        );
        // Even a tampered durable record cannot grant ownership of another PTY.
        let mut forged = own.clone();
        forged.terminal_ids.push(theirs.terminal_id.clone());
        forged.sync_primary();
        f.service.store.put(forged).unwrap();
        assert_eq!(
            f.service.close(own.id).await.unwrap_err().code(),
            "QUICK_TERMINAL_INVALID_TARGET"
        );
        assert_eq!(
            f.service.delete(own.id).await.unwrap_err().code(),
            "QUICK_TERMINAL_INVALID_TARGET"
        );
        assert!(f.service.shell_is_running(&theirs.terminal_id));
        assert!(f.service.get(own.id).unwrap().terminal_ids.is_empty());
        f.service.delete(foreign.id).await.unwrap();
    }

    #[tokio::test]
    async fn dead_target_is_pruned_without_replacing_or_affecting_other_live_shells() {
        let f = fixture();
        let record = entry(&f);
        let first = f.service.open(open(record.id)).await.unwrap();
        let second = f.service.create_terminal(record.id, 80, 24).await.unwrap();
        f.pty.terminate(&first.terminal_id).await.unwrap();
        assert_eq!(
            f.service.list()[0].terminal_ids,
            vec![second.terminal_id.clone()]
        );
        let mut request = open(record.id);
        request.terminal_id = Some(first.terminal_id.clone());
        assert_eq!(
            f.service.open(request).await.unwrap_err().code(),
            "QUICK_TERMINAL_INVALID_TARGET"
        );
        let reused = f.service.open(open(record.id)).await.unwrap();
        assert!(!reused.spawned);
        assert_eq!(reused.terminal_id, second.terminal_id);
        f.service.delete(record.id).await.unwrap();
    }

    #[tokio::test]
    async fn persist_failure_rolls_back_only_the_newly_spawned_shell() {
        let f = fixture();
        let record = entry(&f);
        let first = f.service.open(open(record.id)).await.unwrap();
        let dir = f
            ._state
            .path()
            .canonicalize()
            .unwrap()
            .join("quick-terminals");
        let path = dir.join(format!("{}.json", record.id));
        // Durable writer refuses a symlink destination. No live user state touched.
        std::fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(dir.join("blocked"), &path).unwrap();
        let error = f
            .service
            .create_terminal(record.id, 80, 24)
            .await
            .unwrap_err();
        assert_eq!(error.code(), "QUICK_TERMINAL_STORAGE_FAILED");
        assert!(f.service.shell_is_running(&first.terminal_id));
        assert_eq!(
            f.pty.get_by_conversation(record.id).len(),
            1,
            "failed spawn is rolled back"
        );
        assert_eq!(
            f.service.store.get(record.id).unwrap().terminal_ids,
            vec![first.terminal_id.clone()]
        );
        std::fs::remove_file(path).unwrap();
        f.service.delete(record.id).await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_creates_and_opens_preserve_all_owned_shells() {
        let f = fixture();
        let record = entry(&f);
        let service = Arc::new(f.service);
        let mut tasks = Vec::new();
        for _ in 0..3 {
            let service = Arc::clone(&service);
            tasks.push(tokio::spawn(async move {
                service.create_terminal(record.id, 80, 24).await.unwrap()
            }));
        }
        let mut ids = Vec::new();
        for task in tasks {
            ids.push(task.await.unwrap().terminal_id);
        }
        assert_eq!(service.get(record.id).unwrap().terminal_ids.len(), 3);
        assert!(ids.iter().all(|id| service.shell_is_running(id)));
        let reused = service.open(open(record.id)).await.unwrap();
        assert!(!reused.spawned && ids.contains(&reused.terminal_id));
        service.delete(record.id).await.unwrap();
        assert!(ids.iter().all(|id| !service.shell_is_running(id)));
    }
    #[tokio::test]
    async fn failed_rollback_retains_ownership_and_delete_retries_cleanup() {
        let f = fixture();
        let record = entry(&f);
        let dir = f
            ._state
            .path()
            .canonicalize()
            .unwrap()
            .join("quick-terminals");
        let path = dir.join(format!("{}.json", record.id));
        std::fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(dir.join("blocked"), &path).unwrap();
        f.service.fail_cleanup.lock().insert("*".into());
        assert_eq!(
            f.service
                .create_terminal(record.id, 80, 24)
                .await
                .unwrap_err()
                .code(),
            "QUICK_TERMINAL_TERMINATE_FAILED"
        );
        let owned = f.service.stored_record(record.id).unwrap().terminal_ids;
        assert_eq!(owned.len(), 1);
        assert!(f.service.delete(record.id).await.is_err());
        assert!(f.service.store.get(record.id).is_some());
        // New opens cannot compound the failed rollback.
        assert!(f.service.open(open(record.id)).await.is_err());
        assert_eq!(f.pty.get_by_conversation(record.id).len(), 1);
        f.service.fail_cleanup.lock().clear();
        std::fs::remove_file(path).unwrap();
        f.service.delete(record.id).await.unwrap();
        assert!(!f.service.shell_is_running(&owned[0]));
        assert!(f.service.pending_cleanup.lock().get(&record.id).is_none());
    }

    #[tokio::test]
    async fn close_all_attempts_later_siblings_after_cleanup_failure() {
        let f = fixture();
        let record = entry(&f);
        let first = f.service.create_terminal(record.id, 80, 24).await.unwrap();
        let second = f.service.create_terminal(record.id, 80, 24).await.unwrap();
        f.service
            .fail_cleanup
            .lock()
            .insert(first.terminal_id.clone());
        assert!(f.service.close(record.id).await.is_err());
        assert!(f.service.shell_is_running(&first.terminal_id));
        assert!(!f.service.shell_is_running(&second.terminal_id));
        assert!(f.service.store.get(record.id).is_some());
        f.service.fail_cleanup.lock().clear();
        f.service.delete(record.id).await.unwrap();
        assert!(!f.service.shell_is_running(&first.terminal_id));
    }
}
