import type {
  ScheduledTaskApi,
  ScheduledTaskAuditEventV1,
  ScheduledTaskRecordV1,
  ScheduledTaskRunV1,
  ScheduledTaskSchedulePreview
} from '@shared/types/scheduled-task.types'
import {
  parseScheduledTask,
  parseScheduledTaskArray,
  parseScheduledTaskAuditArray,
  parseScheduledTaskPreview,
  parseScheduledTaskRun,
  parseScheduledTaskRunArray
} from '@shared/types/scheduled-task.types'
import { invokeDecodedIpcResult } from './invoke-decoded-ipc-result'

const decodeVoid = (): void => undefined

export function createTauriScheduledTaskApi(): ScheduledTaskApi {
  return {
    previewSchedule: (schedule) =>
      invokeDecodedIpcResult<ScheduledTaskSchedulePreview>(
        'scheduled_task_preview',
        parseScheduledTaskPreview,
        { schedule, count: 5 }
      ),
    listTasks: (projectId) =>
      invokeDecodedIpcResult<ScheduledTaskRecordV1[]>(
        'scheduled_task_list',
        parseScheduledTaskArray,
        { projectId }
      ),
    getTask: (taskId) =>
      invokeDecodedIpcResult('scheduled_task_get', parseScheduledTask, { taskId }),
    createDraft: (input) =>
      invokeDecodedIpcResult('scheduled_task_draft_create', parseScheduledTask, { input }),
    updateDraft: (taskId, expectedRevision, input) =>
      invokeDecodedIpcResult('scheduled_task_draft_update', parseScheduledTask, {
        taskId,
        request: { expectedRevision, input }
      }),
    activateTask: (taskId, expectedRevision, expectedDraftHash) =>
      invokeDecodedIpcResult('scheduled_task_activate', parseScheduledTask, {
        taskId,
        request: { expectedRevision, expectedDraftHash }
      }),
    pauseTask: (taskId, expectedRevision) =>
      invokeDecodedIpcResult('scheduled_task_pause', parseScheduledTask, {
        taskId,
        request: { expectedRevision }
      }),
    resumeTask: (taskId, expectedRevision) =>
      invokeDecodedIpcResult('scheduled_task_resume', parseScheduledTask, {
        taskId,
        request: { expectedRevision }
      }),
    deleteTask: (taskId, expectedRevision) =>
      invokeDecodedIpcResult<void>('scheduled_task_delete', decodeVoid, {
        taskId,
        request: { expectedRevision }
      }),
    runNow: (taskId) =>
      invokeDecodedIpcResult<ScheduledTaskRunV1>('scheduled_task_run_now', parseScheduledTaskRun, {
        taskId
      }),
    retryRun: (taskId, runId) =>
      invokeDecodedIpcResult<ScheduledTaskRunV1>(
        'scheduled_task_retry_run',
        parseScheduledTaskRun,
        { taskId, runId }
      ),
    listRuns: (taskId) =>
      invokeDecodedIpcResult<ScheduledTaskRunV1[]>(
        'scheduled_task_list_runs',
        parseScheduledTaskRunArray,
        { taskId }
      ),
    listAudit: (taskId) =>
      invokeDecodedIpcResult<ScheduledTaskAuditEventV1[]>(
        'scheduled_task_list_audit',
        parseScheduledTaskAuditArray,
        { taskId }
      )
  }
}

export const tauriScheduledTaskApi = createTauriScheduledTaskApi()
