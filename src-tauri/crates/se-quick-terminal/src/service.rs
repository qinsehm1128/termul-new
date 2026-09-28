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
}

impl QuickTerminalService {
    pub fn new(store: QuickTerminalStore, workspace_base: PathBuf, pty: Arc<PtyManager>) -> Self {
        Self {
            store,
            workspace_base,
            pty,
            fs: DurableFileSystem::new(),
            locks: parking_lot::Mutex::new(HashMap::new()),
        }
    }

    pub fn list(&self) -> Vec<QuickTerminalRecord> {
        self.store.list()
    }

    pub fn get(&self, id: QuickTerminalId) -> Result<QuickTerminalRecord> {
        self.store.get(id).ok_or(QuickTerminalError::NotFound)
    }

    pub fn create(&self, request: CreateQuickTerminal) -> Result<QuickTerminalRecord> {
        let id = QuickTerminalId::new_v4();
        let now = Utc::now();
        let cwd = match &request.target {
            QuickTerminalTarget::Workspace => {
                let folder = self.workspace_base.join("terminals").join(format!(
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

    /// Reuse the quick terminal's live PTY, or start a new shell in its folder.
    pub async fn open(&self, request: OpenQuickTerminal) -> Result<QuickTerminalOpened> {
        let lock = self.lock_for(request.id);
        let _guard = lock.lock().await;
        let mut record = self.get(request.id)?;
        if let Some(terminal_id) = record.terminal_id.clone() {
            if self.shell_is_running(&terminal_id) {
                return Ok(QuickTerminalOpened {
                    record,
                    terminal_id,
                    claim: None,
                    spawned: false,
                });
            }
            // A shell that exited on its own stays registered until someone
            // cleans it up; release it before starting the replacement.
            if self.pty.get(&terminal_id).is_some() {
                if let Err(failure) = self.pty.terminate(&terminal_id).await {
                    log::warn!(
                        target: "se_manager::quick_terminal",
                        "operation=open id={} terminal_id={terminal_id} stable_code=STALE_PTY_CLEANUP_FAILED detail={failure:?}",
                        record.id
                    );
                }
            }
        }
        if request.cols == 0 || request.rows == 0 {
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
            // The user removed the private folder; give the shell a fresh one.
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
                    project_id: record.target.project_id().map(str::to_owned),
                    cols: Some(request.cols),
                    rows: Some(request.rows),
                    ..Default::default()
                },
                None,
            )
            .await
            .map_err(QuickTerminalError::Spawn)?;
        let terminal_id = spawned.info.id.clone();
        record.terminal_id = Some(terminal_id.clone());
        record.updated_at_utc = timestamp(Utc::now());
        self.store.put(record.clone()).map_err(storage)?;
        log::info!(
            target: "se_manager::quick_terminal",
            "operation=open id={} terminal_id={terminal_id} spawned=true stable_code=OK",
            record.id
        );
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
        let mut record = self.get(id)?;
        record.title = normalize_title(title);
        record.updated_at_utc = timestamp(Utc::now());
        self.store.put(record.clone()).map_err(storage)?;
        Ok(record)
    }

    /// End the shell and forget the quick terminal. Its folder is kept: it may
    /// hold the user's files.
    pub async fn delete(&self, id: QuickTerminalId) -> Result<()> {
        let lock = self.lock_for(id);
        let _guard = lock.lock().await;
        let record = self.get(id)?;
        if let Some(terminal_id) = record.terminal_id.as_deref() {
            if self.pty.get(terminal_id).is_some() {
                self.pty.terminate(terminal_id).await.map_err(|failure| {
                    log::error!(
                        target: "se_manager::quick_terminal",
                        "operation=delete id={id} terminal_id={terminal_id} stable_code=QUICK_TERMINAL_TERMINATE_FAILED"
                    );
                    QuickTerminalError::Terminate(format!("{failure:?}"))
                })?;
            }
        }
        self.store.remove(id).map_err(storage)?;
        self.locks.lock().remove(&id);
        log::info!(
            target: "se_manager::quick_terminal",
            "operation=delete id={id} stable_code=OK"
        );
        Ok(())
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

    fn lock_for(&self, id: QuickTerminalId) -> Arc<tokio::sync::Mutex<()>> {
        Arc::clone(self.locks.lock().entry(id).or_default())
    }
}

fn storage(error: std::io::Error) -> QuickTerminalError {
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
}
