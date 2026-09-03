use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use tokio::sync::{watch, Notify, Semaphore};

use super::models::{
    CatchUpPolicy, OverlapPolicy, ScheduledTaskAuditEventV1, ScheduledTaskDraftInputV1,
    ScheduledTaskRunStatus, ScheduledTaskRunTrigger, ScheduledTaskRunV1, ScheduledTaskStatus,
    ScheduledTaskV1, TaskMutationContextV1,
};
use super::runner::{execute_and_record, ScheduledTaskExecutor};
use super::schedule::{next_after, occurrences_after, preview_schedule};
use super::store::{new_queued_run, ScheduledTaskStore, ScheduledTaskStoreError};

const DEFAULT_MAX_CONCURRENT_RUNS: usize = 3;
const MAX_CATCH_UP_OCCURRENCES: usize = 4096;

pub struct ScheduledTaskService {
    store: Arc<ScheduledTaskStore>,
    executor: Arc<dyn ScheduledTaskExecutor>,
    notify: Notify,
    shutdown_tx: watch::Sender<bool>,
    workers: Arc<Semaphore>,
    running_tasks: Mutex<HashSet<String>>,
    buffered_runs: Mutex<HashMap<String, (ScheduledTaskV1, ScheduledTaskRunV1)>>,
}

impl ScheduledTaskService {
    #[must_use]
    pub fn new(
        store: Arc<ScheduledTaskStore>,
        executor: Arc<dyn ScheduledTaskExecutor>,
    ) -> Arc<Self> {
        Self::with_max_concurrent_runs(store, executor, DEFAULT_MAX_CONCURRENT_RUNS)
    }

    #[must_use]
    pub fn with_max_concurrent_runs(
        store: Arc<ScheduledTaskStore>,
        executor: Arc<dyn ScheduledTaskExecutor>,
        max_concurrent_runs: usize,
    ) -> Arc<Self> {
        let (shutdown_tx, _) = watch::channel(false);
        Arc::new(Self {
            store,
            executor,
            notify: Notify::new(),
            shutdown_tx,
            workers: Arc::new(Semaphore::new(max_concurrent_runs.max(1))),
            running_tasks: Mutex::new(HashSet::new()),
            buffered_runs: Mutex::new(HashMap::new()),
        })
    }

    pub fn start(self: &Arc<Self>) {
        self.start_on(&tokio::runtime::Handle::current());
    }

    pub fn start_on(self: &Arc<Self>, runtime: &tokio::runtime::Handle) {
        let service = Arc::clone(self);
        runtime.spawn(async move {
            service.recover_interrupted_runs();
            service.catch_up_after_start().await;
            service.scheduler_loop().await;
        });
    }

    pub async fn shutdown(&self, timeout: Duration) {
        let _ = self.shutdown_tx.send(true);
        self.notify.notify_waiters();
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let running = self
                .running_tasks
                .lock()
                .map(|tasks| tasks.is_empty())
                .unwrap_or(true);
            if running || tokio::time::Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    #[must_use]
    pub fn store(&self) -> Arc<ScheduledTaskStore> {
        Arc::clone(&self.store)
    }

    pub fn preview(
        &self,
        schedule: &super::models::ScheduleSpecV1,
        count: usize,
    ) -> Result<super::models::SchedulePreviewV1, ScheduledTaskStoreError> {
        preview_schedule(schedule, Utc::now(), count).map_err(Into::into)
    }

    pub fn list_tasks(
        &self,
        project_id: Option<&str>,
    ) -> Result<Vec<ScheduledTaskV1>, ScheduledTaskStoreError> {
        self.store.list_tasks(project_id)
    }

    pub fn get_task(&self, task_id: &str) -> Result<ScheduledTaskV1, ScheduledTaskStoreError> {
        self.store.get_task(task_id)
    }

    pub fn create_draft(
        &self,
        input: ScheduledTaskDraftInputV1,
        context: TaskMutationContextV1,
    ) -> Result<ScheduledTaskV1, ScheduledTaskStoreError> {
        let task = self.store.create_draft(input, context)?;
        log::info!(
            "[scheduled-task] boundary=draft_created task_id={} project_id={:?}",
            task.task_id,
            task.project_id
        );
        self.notify.notify_one();
        Ok(task)
    }

    pub fn update_draft(
        &self,
        task_id: &str,
        expected_revision: u64,
        input: ScheduledTaskDraftInputV1,
        context: TaskMutationContextV1,
    ) -> Result<ScheduledTaskV1, ScheduledTaskStoreError> {
        let task = self
            .store
            .update_draft(task_id, expected_revision, input, context)?;
        self.notify.notify_one();
        Ok(task)
    }

    pub fn activate(
        &self,
        task_id: &str,
        expected_revision: u64,
        expected_draft_hash: &str,
        context: TaskMutationContextV1,
    ) -> Result<ScheduledTaskV1, ScheduledTaskStoreError> {
        let task = self
            .store
            .activate(task_id, expected_revision, expected_draft_hash, context)?;
        log::info!(
            "[scheduled-task] boundary=activated task_id={} project_id={:?}",
            task.task_id,
            task.project_id
        );
        self.notify.notify_one();
        Ok(task)
    }

    pub fn pause(
        &self,
        task_id: &str,
        expected_revision: u64,
        context: TaskMutationContextV1,
    ) -> Result<ScheduledTaskV1, ScheduledTaskStoreError> {
        let task = self.store.pause(task_id, expected_revision, context)?;
        log::info!(
            "[scheduled-task] boundary=paused task_id={} project_id={:?}",
            task.task_id,
            task.project_id
        );
        self.notify.notify_one();
        Ok(task)
    }

    pub fn resume(
        &self,
        task_id: &str,
        expected_revision: u64,
        context: TaskMutationContextV1,
    ) -> Result<ScheduledTaskV1, ScheduledTaskStoreError> {
        let task = self.store.resume(task_id, expected_revision, context)?;
        log::info!(
            "[scheduled-task] boundary=resumed task_id={} project_id={:?}",
            task.task_id,
            task.project_id
        );
        self.notify.notify_one();
        Ok(task)
    }

    pub fn delete(
        &self,
        task_id: &str,
        expected_revision: u64,
        context: TaskMutationContextV1,
    ) -> Result<(), ScheduledTaskStoreError> {
        self.store.delete(task_id, expected_revision, context)?;
        self.notify.notify_one();
        Ok(())
    }

    pub fn list_runs(
        &self,
        task_id: &str,
    ) -> Result<Vec<ScheduledTaskRunV1>, ScheduledTaskStoreError> {
        self.store.list_runs(task_id)
    }

    pub fn list_audit(
        &self,
        task_id: &str,
    ) -> Result<Vec<ScheduledTaskAuditEventV1>, ScheduledTaskStoreError> {
        self.store.list_audit(task_id)
    }

    pub fn run_now(
        self: &Arc<Self>,
        task_id: &str,
    ) -> Result<ScheduledTaskRunV1, ScheduledTaskStoreError> {
        let task = self.store.get_task(task_id)?;
        ensure_task_active(&task)?;
        let run = new_queued_run(
            &task,
            ScheduledTaskRunTrigger::Manual,
            Utc::now().to_rfc3339(),
            None,
        )?;
        self.queue_or_overlap(task, run.clone())?;
        Ok(run)
    }

    pub fn retry_run(
        self: &Arc<Self>,
        task_id: &str,
        retry_of_run_id: &str,
    ) -> Result<ScheduledTaskRunV1, ScheduledTaskStoreError> {
        let task = self.store.get_task(task_id)?;
        ensure_task_active(&task)?;
        let previous = self
            .store
            .list_runs(task_id)?
            .into_iter()
            .find(|run| run.run_id == retry_of_run_id)
            .ok_or_else(|| ScheduledTaskStoreError::NotFound(retry_of_run_id.to_string()))?;
        if matches!(
            previous.status,
            ScheduledTaskRunStatus::Queued | ScheduledTaskRunStatus::Running
        ) {
            return Err(ScheduledTaskStoreError::InvalidInput(
                "a queued or running attempt cannot be retried".to_string(),
            ));
        }
        let run = new_queued_run(
            &task,
            ScheduledTaskRunTrigger::Retry,
            Utc::now().to_rfc3339(),
            Some(previous.run_id),
        )?;
        self.queue_or_overlap(task, run.clone())?;
        Ok(run)
    }

    async fn scheduler_loop(self: Arc<Self>) {
        let mut shutdown_rx = self.shutdown_tx.subscribe();
        loop {
            if *shutdown_rx.borrow() {
                break;
            }
            let next_due = self.process_due_tasks();
            match next_due {
                Some(next_due) => {
                    let wait = (next_due - Utc::now())
                        .to_std()
                        .unwrap_or(Duration::from_millis(1));
                    tokio::select! {
                        _ = tokio::time::sleep(wait) => {}
                        _ = self.notify.notified() => {}
                        _ = shutdown_rx.changed() => {}
                    }
                }
                None => {
                    tokio::select! {
                        _ = self.notify.notified() => {}
                        _ = shutdown_rx.changed() => {}
                    }
                }
            }
        }
        log::info!("[scheduled-task] boundary=scheduler_stopped");
    }

    fn process_due_tasks(self: &Arc<Self>) -> Option<DateTime<Utc>> {
        let now = Utc::now();
        let tasks = match self.store.list_tasks(None) {
            Ok(tasks) => tasks,
            Err(error) => {
                log::error!(
                    "[scheduled-task] boundary=schedule_reload code=TASK_STORE_READ_FAILED error={error}"
                );
                return Some(now + ChronoDuration::seconds(30));
            }
        };
        let mut earliest = None;
        for task in tasks
            .into_iter()
            .filter(|task| task.status == ScheduledTaskStatus::Active)
        {
            let due = task
                .next_run_at
                .as_deref()
                .and_then(parse_utc)
                .or_else(|| next_after(&task.schedule, now).ok().flatten());
            let Some(due) = due else {
                continue;
            };
            if due <= now {
                if !self.occurrence_exists(&task.task_id, due) {
                    match new_queued_run(
                        &task,
                        ScheduledTaskRunTrigger::Scheduled,
                        due.to_rfc3339(),
                        None,
                    )
                    .and_then(|run| self.queue_or_overlap(task.clone(), run))
                    {
                        Ok(()) => log::info!(
                            "[scheduled-task] boundary=due task_id={} scheduled_for={}",
                            task.task_id,
                            due.to_rfc3339()
                        ),
                        Err(error) => log::error!(
                            "[scheduled-task] boundary=queue task_id={} code=TASK_QUEUE_FAILED error={error}",
                            task.task_id
                        ),
                    }
                }
                let next = next_future_after_now(&task, now);
                if let Err(error) = self
                    .store
                    .set_next_run_at(&task.task_id, next.map(|value| value.to_rfc3339()))
                {
                    log::error!(
                        "[scheduled-task] boundary=advance task_id={} code=TASK_STORE_WRITE_FAILED error={error}",
                        task.task_id
                    );
                }
                if let Some(next) = next {
                    earliest = min_time(earliest, next);
                }
            } else {
                earliest = min_time(earliest, due);
            }
        }
        earliest
    }

    fn queue_or_overlap(
        self: &Arc<Self>,
        task: ScheduledTaskV1,
        run: ScheduledTaskRunV1,
    ) -> Result<(), ScheduledTaskStoreError> {
        self.store.append_run(&run)?;
        let is_running = self
            .running_tasks
            .lock()
            .map_err(|_| ScheduledTaskStoreError::Poisoned)?
            .contains(&task.task_id);
        if is_running {
            match task.execution_policy.overlap {
                OverlapPolicy::Skip => {
                    let mut skipped = run;
                    skipped.status = ScheduledTaskRunStatus::Skipped;
                    skipped.finished_at = Some(Utc::now().to_rfc3339());
                    skipped.error_code = Some("OVERLAP_SKIPPED".to_string());
                    skipped.error_detail =
                        Some("another run of this task is still active".to_string());
                    self.store.append_run(&skipped)?;
                    log::info!(
                        "[scheduled-task] boundary=overlap task_id={} run_id={} policy=skip",
                        task.task_id,
                        skipped.run_id
                    );
                }
                OverlapPolicy::BufferOne => {
                    let previous = self
                        .buffered_runs
                        .lock()
                        .map_err(|_| ScheduledTaskStoreError::Poisoned)?
                        .insert(task.task_id.clone(), (task.clone(), run.clone()));
                    if let Some((_, mut previous)) = previous {
                        previous.status = ScheduledTaskRunStatus::Skipped;
                        previous.finished_at = Some(Utc::now().to_rfc3339());
                        previous.error_code = Some("BUFFER_SUPERSEDED".to_string());
                        previous.error_detail =
                            Some("a newer overlapping occurrence replaced this buffer".to_string());
                        self.store.append_run(&previous)?;
                    }
                    log::info!(
                        "[scheduled-task] boundary=overlap task_id={} run_id={} policy=buffer_one",
                        task.task_id,
                        run.run_id
                    );
                }
            }
            return Ok(());
        }
        self.dispatch(task, run)
    }

    fn dispatch(
        self: &Arc<Self>,
        task: ScheduledTaskV1,
        run: ScheduledTaskRunV1,
    ) -> Result<(), ScheduledTaskStoreError> {
        self.running_tasks
            .lock()
            .map_err(|_| ScheduledTaskStoreError::Poisoned)?
            .insert(task.task_id.clone());
        let service = Arc::clone(self);
        tokio::spawn(async move {
            let permit = match Arc::clone(&service.workers).acquire_owned().await {
                Ok(permit) => permit,
                Err(_) => return,
            };
            let task_id = task.task_id.clone();
            execute_and_record(
                Arc::clone(&service.store),
                Arc::clone(&service.executor),
                task,
                run,
            )
            .await;
            drop(permit);
            if let Ok(mut running) = service.running_tasks.lock() {
                running.remove(&task_id);
            }
            let buffered = service
                .buffered_runs
                .lock()
                .ok()
                .and_then(|mut buffered| buffered.remove(&task_id));
            if let Some((task, run)) = buffered {
                if let Err(error) = service.dispatch(task, run) {
                    log::error!(
                        "[scheduled-task] boundary=buffer_dispatch task_id={} code=TASK_DISPATCH_FAILED error={error}",
                        task_id
                    );
                }
            }
        });
        Ok(())
    }

    fn occurrence_exists(&self, task_id: &str, scheduled_for: DateTime<Utc>) -> bool {
        let key = format!("{}:{}", task_id, scheduled_for.to_rfc3339());
        self.store
            .list_runs(task_id)
            .map(|runs| runs.iter().any(|run| run.occurrence_key == key))
            .unwrap_or(false)
    }

    fn recover_interrupted_runs(&self) {
        let Ok(tasks) = self.store.list_tasks(None) else {
            return;
        };
        for task in tasks {
            let Ok(runs) = self.store.list_runs(&task.task_id) else {
                continue;
            };
            for mut run in runs.into_iter().filter(|run| {
                matches!(
                    run.status,
                    ScheduledTaskRunStatus::Queued | ScheduledTaskRunStatus::Running
                )
            }) {
                run.status = ScheduledTaskRunStatus::Interrupted;
                run.finished_at = Some(Utc::now().to_rfc3339());
                run.error_code = Some("HOST_RESTART_INTERRUPTED".to_string());
                run.error_detail = Some(
                    "host restarted before completion; retry explicitly to avoid duplicate side effects"
                        .to_string(),
                );
                if let Err(error) = self.store.append_run(&run) {
                    log::error!(
                        "[scheduled-task] boundary=recovery task_id={} run_id={} code=RUN_LEDGER_WRITE_FAILED error={error}",
                        task.task_id,
                        run.run_id
                    );
                } else {
                    log::warn!(
                        "[scheduled-task] boundary=recovery task_id={} run_id={} status=interrupted",
                        task.task_id,
                        run.run_id
                    );
                }
            }
        }
    }

    async fn catch_up_after_start(self: &Arc<Self>) {
        let now = Utc::now();
        let Ok(tasks) = self.store.list_tasks(None) else {
            return;
        };
        for task in tasks
            .into_iter()
            .filter(|task| task.status == ScheduledTaskStatus::Active)
        {
            let Some(stored_next) = task.next_run_at.as_deref().and_then(parse_utc) else {
                continue;
            };
            if stored_next > now {
                continue;
            }
            let window_start =
                now - ChronoDuration::seconds(task.execution_policy.catch_up_window_seconds as i64);
            let latest = occurrences_after(
                &task.schedule,
                window_start - ChronoDuration::milliseconds(1),
                MAX_CATCH_UP_OCCURRENCES,
            )
            .ok()
            .and_then(|values| values.into_iter().take_while(|value| *value <= now).last());
            if let Some(latest) = latest {
                if task.execution_policy.catch_up == CatchUpPolicy::LatestOnce
                    && !self.occurrence_exists(&task.task_id, latest)
                {
                    if let Ok(run) = new_queued_run(
                        &task,
                        ScheduledTaskRunTrigger::CatchUp,
                        latest.to_rfc3339(),
                        None,
                    ) {
                        let _ = self.queue_or_overlap(task.clone(), run);
                        log::info!(
                            "[scheduled-task] boundary=catch_up task_id={} scheduled_for={}",
                            task.task_id,
                            latest.to_rfc3339()
                        );
                    }
                }
            }
            let next = next_future_after_now(&task, now);
            let _ = self
                .store
                .set_next_run_at(&task.task_id, next.map(|value| value.to_rfc3339()));
        }
    }
}

fn next_future_after_now(task: &ScheduledTaskV1, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    next_after(&task.schedule, now).ok().flatten()
}

fn ensure_task_active(task: &ScheduledTaskV1) -> Result<(), ScheduledTaskStoreError> {
    if task.status != ScheduledTaskStatus::Active {
        return Err(ScheduledTaskStoreError::InvalidInput(
            "only an active scheduled task can execute".to_string(),
        ));
    }
    Ok(())
}

fn parse_utc(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|value| value.with_timezone(&Utc))
}

fn min_time(current: Option<DateTime<Utc>>, candidate: DateTime<Utc>) -> Option<DateTime<Utc>> {
    Some(current.map_or(candidate, |current| current.min(candidate)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::path::Path;
    use uuid::Uuid;

    use crate::conversation::ExecutionTarget;

    use super::super::models::{ExecutionPolicyV1, ScheduleSpecV1};
    use super::super::runner::{TaskExecutionError, TaskExecutionOutcome};

    struct SuccessExecutor;

    struct GateExecutor {
        started: tokio::sync::Notify,
        release: tokio::sync::Notify,
    }

    #[async_trait]
    impl ScheduledTaskExecutor for SuccessExecutor {
        async fn execute(
            &self,
            _task: ScheduledTaskV1,
            _run: ScheduledTaskRunV1,
        ) -> Result<TaskExecutionOutcome, TaskExecutionError> {
            Ok(TaskExecutionOutcome {
                conversation_id: None,
                summary: Some("done".to_string()),
                usage: None,
            })
        }
    }

    #[async_trait]
    impl ScheduledTaskExecutor for GateExecutor {
        async fn execute(
            &self,
            _task: ScheduledTaskV1,
            _run: ScheduledTaskRunV1,
        ) -> Result<TaskExecutionOutcome, TaskExecutionError> {
            self.started.notify_one();
            self.release.notified().await;
            Ok(TaskExecutionOutcome {
                conversation_id: None,
                summary: Some("released".to_string()),
                usage: None,
            })
        }
    }

    fn input(root: &Path, overlap: OverlapPolicy) -> ScheduledTaskDraftInputV1 {
        ScheduledTaskDraftInputV1 {
            project_id: Some("project-scheduler".to_string()),
            name: "One shot".to_string(),
            description: String::new(),
            schedule: ScheduleSpecV1::At {
                at: (Utc::now() + ChronoDuration::hours(1)).to_rfc3339(),
            },
            execution_policy: ExecutionPolicyV1 {
                overlap,
                catch_up: CatchUpPolicy::LatestOnce,
                catch_up_window_seconds: 86_400,
            },
            prompt: "Run".to_string(),
            agent_config_id: "test".to_string(),
            execution_target: ExecutionTarget::ProjectRoot {
                project_id: "project-scheduler".to_string(),
                project_root: root.to_string_lossy().into_owned(),
            },
            execution_cwd: root.to_string_lossy().into_owned(),
            workspace_cwd: root.to_string_lossy().into_owned(),
            source_conversation_id: None,
            permissions: Vec::new(),
        }
    }

    #[tokio::test]
    async fn manual_run_is_persisted_and_completed() {
        let root = std::fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!("se-manager-scheduler-{}", Uuid::new_v4()));
        let store = Arc::new(ScheduledTaskStore::open(root.join("state")).unwrap());
        let service = ScheduledTaskService::with_max_concurrent_runs(
            Arc::clone(&store),
            Arc::new(SuccessExecutor),
            1,
        );
        let task = service
            .create_draft(input(&root, OverlapPolicy::BufferOne), Default::default())
            .unwrap();
        let task = service
            .activate(
                &task.task_id,
                task.revision,
                &task.draft_hash,
                Default::default(),
            )
            .unwrap();
        service.run_now(&task.task_id).unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        let runs = service.list_runs(&task.task_id).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].status, ScheduledTaskRunStatus::Succeeded);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn draft_cannot_run_before_explicit_activation() {
        let root = std::fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!("se-manager-scheduler-draft-{}", Uuid::new_v4()));
        let store = Arc::new(ScheduledTaskStore::open(root.join("state")).unwrap());
        let service = ScheduledTaskService::with_max_concurrent_runs(
            Arc::clone(&store),
            Arc::new(SuccessExecutor),
            1,
        );
        let task = service
            .create_draft(input(&root, OverlapPolicy::BufferOne), Default::default())
            .unwrap();
        assert!(matches!(
            service.run_now(&task.task_id),
            Err(ScheduledTaskStoreError::InvalidInput(detail))
                if detail.contains("only an active")
        ));
        assert!(service.list_runs(&task.task_id).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn startup_recovery_marks_queued_runs_interrupted_without_reexecution() {
        let root = std::fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!("se-manager-scheduler-recovery-{}", Uuid::new_v4()));
        let store = Arc::new(ScheduledTaskStore::open(root.join("state")).unwrap());
        let service = ScheduledTaskService::with_max_concurrent_runs(
            Arc::clone(&store),
            Arc::new(SuccessExecutor),
            1,
        );
        let task = service
            .create_draft(input(&root, OverlapPolicy::BufferOne), Default::default())
            .unwrap();
        let run = new_queued_run(
            &task,
            ScheduledTaskRunTrigger::Manual,
            Utc::now().to_rfc3339(),
            None,
        )
        .unwrap();
        store.append_run(&run).unwrap();

        service.recover_interrupted_runs();

        let recovered = store.list_runs(&task.task_id).unwrap();
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].status, ScheduledTaskRunStatus::Interrupted);
        assert_eq!(
            recovered[0].error_code.as_deref(),
            Some("HOST_RESTART_INTERRUPTED")
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn startup_catch_up_enqueues_only_latest_missed_occurrence() {
        let root = std::fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!("se-manager-scheduler-catchup-{}", Uuid::new_v4()));
        let store = Arc::new(ScheduledTaskStore::open(root.join("state")).unwrap());
        let service = ScheduledTaskService::with_max_concurrent_runs(
            Arc::clone(&store),
            Arc::new(SuccessExecutor),
            1,
        );
        let mut draft = input(&root, OverlapPolicy::BufferOne);
        draft.schedule = ScheduleSpecV1::Interval {
            every_seconds: 60 * 60,
            anchor_at: (Utc::now() - ChronoDuration::hours(3)).to_rfc3339(),
        };
        let task = service.create_draft(draft, Default::default()).unwrap();
        let task = service
            .activate(
                &task.task_id,
                task.revision,
                &task.draft_hash,
                Default::default(),
            )
            .unwrap();
        store
            .set_next_run_at(
                &task.task_id,
                Some((Utc::now() - ChronoDuration::hours(2)).to_rfc3339()),
            )
            .unwrap();

        service.catch_up_after_start().await;
        tokio::time::sleep(Duration::from_millis(50)).await;

        let runs = store.list_runs(&task.task_id).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].trigger, ScheduledTaskRunTrigger::CatchUp);
        assert_eq!(runs[0].status, ScheduledTaskRunStatus::Succeeded);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn skip_overlap_records_second_occurrence_without_executing_it() {
        let root = std::fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!("se-manager-scheduler-overlap-{}", Uuid::new_v4()));
        let store = Arc::new(ScheduledTaskStore::open(root.join("state")).unwrap());
        let executor = Arc::new(GateExecutor {
            started: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
        });
        let service =
            ScheduledTaskService::with_max_concurrent_runs(Arc::clone(&store), executor.clone(), 1);
        let task = service
            .create_draft(input(&root, OverlapPolicy::Skip), Default::default())
            .unwrap();
        let task = service
            .activate(
                &task.task_id,
                task.revision,
                &task.draft_hash,
                Default::default(),
            )
            .unwrap();

        service.run_now(&task.task_id).unwrap();
        executor.started.notified().await;
        service.run_now(&task.task_id).unwrap();
        executor.release.notify_one();
        tokio::time::sleep(Duration::from_millis(50)).await;

        let runs = store.list_runs(&task.task_id).unwrap();
        assert_eq!(runs.len(), 2);
        assert_eq!(
            runs.iter()
                .filter(|run| run.status == ScheduledTaskRunStatus::Skipped)
                .count(),
            1
        );
        assert_eq!(
            runs.iter()
                .filter(|run| run.status == ScheduledTaskRunStatus::Succeeded)
                .count(),
            1
        );
        let _ = std::fs::remove_dir_all(root);
    }
}
